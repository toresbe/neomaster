//! Connect to MA, print players, queues and library counts, then tail
//! events. `MA_URL` and `MA_TOKEN` come from the environment.
//!
//!     MA_URL=http://192.168.2.238:8095 MA_TOKEN=... cargo run --example ma_probe -- [seconds]

use ma_client::{Client, Config, Event, MediaType, NowPlaying};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
    let token = std::env::var("MA_TOKEN")?;
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);

    let (client, mut events) = Client::connect(Config::new(url, token));
    let info = client.wait_connected().await;
    println!("server {} v{} schema {}", info.server_id, info.server_version, info.schema_version);

    for mt in [MediaType::Artist, MediaType::Album, MediaType::Track, MediaType::Playlist, MediaType::Radio] {
        println!("{:>10}: {}", mt.collection(), client.library_count(mt).await?);
    }
    for p in client.players().await? {
        println!("player {} {:?} available={} {}", p.player_id, p.display_name, p.available, p.playback_state);
    }
    for p in client.players().await? {
        if let Ok(q) = client.queue(&p.player_id).await {
            let np = NowPlaying::from_queue(&q);
            println!("queue {} [{} items, {}] {} - {} ({})", q.queue_id, q.items, q.state, np.artist, np.title, np.album);
        }
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            ev = events.recv() => match ev {
                Some(Event::Other { event, object_id, .. }) => println!("event {event} {object_id:?}"),
                Some(Event::QueueTimeUpdated { queue_id, elapsed }) => println!("time {queue_id} {elapsed:.1}"),
                Some(e) => println!("event {}", format!("{e:?}").chars().take(160).collect::<String>()),
                None => break,
            }
        }
    }
    Ok(())
}
