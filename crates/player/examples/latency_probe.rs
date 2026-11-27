//! Measure Music Assistant → Sendspin latency without making a sound.
//!
//! Registers as a Sendspin player with an output that only records when
//! each buffer arrives and when the clock-synced schedule says it would be
//! heard, then drives MA through `ma-client`: play a URL, skip to the next
//! queue item, repeat. Prints, per trial, the time from the MA command to
//! the first buffer arriving and to its scheduled playback.
//!
//!     MA_TOKEN=... cargo run --release -p player --example latency_probe -- \
//!         <media_url_1> <media_url_2> [trials] [lead_ms]

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sendspin::audio::decode::{Decoder, StreamDecoder};
use sendspin::audio::{AudioBuffer, OutputError, PlayerEvent, PlayerOutput, PlayerSession};
use sendspin::protocol::messages::{AudioFormatSpec, PlayerState, PlayerStateCommand, PlayerV1Support};
use sendspin::sync::ClockSync;
use sendspin::ProtocolClientBuilder;

#[derive(Debug, Clone, Copy)]
enum Mark {
    Start,
    Clear,
    /// arrived, scheduled to play
    Buffer(Instant, Option<Instant>),
}

#[derive(Clone)]
struct ProbeOutput {
    clock: Arc<Mutex<ClockSync>>,
    log: Arc<Mutex<Vec<Mark>>>,
}

impl PlayerOutput for ProbeOutput {
    fn commit(&self, buffer: AudioBuffer) -> Result<(), OutputError> {
        let now = Instant::now();
        let at = self.clock.lock().server_to_local_instant(buffer.timestamp);
        self.log.lock().push(Mark::Buffer(now, at));
        Ok(())
    }
    fn clear(&self) -> Result<(), OutputError> {
        self.log.lock().push(Mark::Clear);
        Ok(())
    }
    fn set_volume(&self, _: u8) -> Result<(), OutputError> {
        Ok(())
    }
    fn set_mute(&self, _: bool) -> Result<(), OutputError> {
        Ok(())
    }
    fn set_static_delay(&self, _: u16) -> Result<(), OutputError> {
        Ok(())
    }
}

fn pcm(rate: u32) -> AudioFormatSpec {
    AudioFormatSpec { codec: "pcm".into(), channels: 2, sample_rate: rate, bit_depth: 16 }
}

/// First buffer after the first stream restart (clear or start) logged
/// at or after index `from`: (arrival - t, scheduled - t).
fn first_after_restart(log: &[Mark], from: usize, t: Instant) -> Option<(Duration, Option<Duration>)> {
    let restart = from + log[from..].iter().position(|m| matches!(m, Mark::Start | Mark::Clear))?;
    log[restart..].iter().find_map(|m| match *m {
        Mark::Buffer(a, s) => Some((a.saturating_duration_since(t), s.map(|s| s.saturating_duration_since(t)))),
        _ => None,
    })
}

/// First buffer logged after `t`: (arrival - t, scheduled - t).
fn first_after(log: &[Mark], t: Instant) -> Option<(Duration, Option<Duration>)> {
    log.iter().find_map(|m| match *m {
        Mark::Buffer(a, s) if a > t => Some((a - t, s.map(|s| s.saturating_duration_since(t)))),
        _ => None,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (url1, url2) = (args[0].clone(), args[1].clone());
    let trials: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let lead_ms: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(500);
    let ma_url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
    let ss_url = std::env::var("SENDSPIN_URL").unwrap_or("ws://192.168.2.238:8927/sendspin".into());
    let client_id = "neomaster-latency-probe".to_string();

    let ss = ProtocolClientBuilder::builder()
        .client_id(client_id.clone())
        .name("neomaster latency probe".to_string())
        .player_v1_support(PlayerV1Support {
            supported_formats: vec![pcm(44_100), pcm(48_000)],
            buffer_capacity: 8 * 1024 * 1024,
            supported_commands: vec!["volume".into(), "mute".into()],
        })
        .initial_player_state(PlayerState {
            volume: Some(100),
            muted: Some(false),
            static_delay_ms: Some(0),
            required_lead_time_ms: Some(lead_ms),
            min_buffer_ms: Some(lead_ms),
            supported_commands: Some(vec![PlayerStateCommand::SetStaticDelay]),
        })
        .build()
        .connect(&ss_url)
        .await?;
    let conn = ss.split();
    let clock = conn.clock_sync.clone();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut session: PlayerSession<ProbeOutput> = conn.player.attach(ProbeOutput { clock: clock.clone(), log: log.clone() });
    let (_artwork, _vis, _msgs, _guard) = (conn.artwork, conn.visualizer, conn.messages, conn.guard);
    let evlog = log.clone();
    tokio::spawn(async move {
        let mut dec: Option<StreamDecoder> = None;
        while let Some(ev) = session.recv().await {
            match ev {
                PlayerEvent::Start(cfg) => {
                    evlog.lock().push(Mark::Start);
                    dec = StreamDecoder::new(&cfg).ok();
                }
                PlayerEvent::Audio(work) => {
                    let Some(d) = &dec else { continue };
                    if let Ok(samples) = d.decode(work.data()) {
                        let buf = AudioBuffer { timestamp: work.timestamp(), samples, format: d.format().clone() };
                        let _ = work.commit(buf);
                    }
                }
                PlayerEvent::Clear => evlog.lock().push(Mark::Clear),
                PlayerEvent::End => dec = None,
                PlayerEvent::OutputLost(e) => log::warn!("output lost: {e}"),
            }
        }
    });

    // Find the MA player that wraps this Sendspin client.
    let (ma, _events) = ma_client::Client::connect(ma_client::Config::new(ma_url, std::env::var("MA_TOKEN")?));
    ma.wait_connected().await;
    let mut queue = None;
    for _ in 0..120 {
        queue = ma.player_for_output(&client_id).await?;
        if queue.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let queue = queue.ok_or("MA did not list the probe player")?;
    // Let the clock filter settle.
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("MA player {queue}, lead {lead_ms} ms, rtt {:?} us", clock.lock().rtt_micros());

    let ms = |d: Duration| format!("{:>5} ms", d.as_millis());
    let mut starts = Vec::new();
    let mut skips = Vec::new();
    for i in 0..trials {
        let t = Instant::now();
        ma.play_media(&queue, &[&url1, &url2], ma_client::QueueOption::Play).await?;
        let reply = t.elapsed();
        tokio::time::sleep(Duration::from_secs(4)).await;
        let s = first_after(&log.lock(), t);
        let from = log.lock().len();
        let t2 = Instant::now();
        ma.next(&queue).await?;
        tokio::time::sleep(Duration::from_secs(4)).await;
        let k = first_after_restart(&log.lock(), from, t2);
        let show = |x: Option<(Duration, Option<Duration>)>| match x {
            Some((a, Some(s))) => format!("first buffer {}  heard at {}", ms(a), ms(s)),
            Some((a, None)) => format!("first buffer {}  (clock not synced)", ms(a)),
            None => "no audio".into(),
        };
        println!("trial {i}: play (reply {}) {} | next {}", ms(reply), show(s), show(k));
        if let Some((_, Some(x))) = s {
            starts.push(x);
        }
        if let Some((_, Some(x))) = k {
            skips.push(x);
        }
        ma.stop(&queue).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let med = |v: &mut Vec<Duration>| {
        v.sort();
        v.get(v.len() / 2).copied()
    };
    println!("median start-to-sound {:?}, skip-to-sound {:?}", med(&mut starts), med(&mut skips));
    Ok(())
}
