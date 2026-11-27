//! Run the Sendspin player on its own and print what it reports, and the
//! MA player id to send `play_media` to.
//!
//!     MA_TOKEN=... cargo run --release -p player --example bm5_player -- \
//!         [client_id] [name] [alsa_device]
//!
//! This plays real audio on the chosen device when MA streams to it.

use player::{Player, PlayerConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let id = args.next().unwrap_or("neomaster-bm5".into());
    let name = args.next().unwrap_or("BeoMaster 5 (neomaster)".into());
    let device = args.next();
    let ss = std::env::var("SENDSPIN_URL").unwrap_or("ws://192.168.2.238:8927/sendspin".into());
    let mut cfg = PlayerConfig::new(ss, id.clone(), name);
    cfg.device = device;
    let (_player, mut events) = Player::spawn(cfg);

    if let Ok(token) = std::env::var("MA_TOKEN") {
        let url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
        let (ma, _ev) = ma_client::Client::connect(ma_client::Config::new(url, token));
        tokio::spawn(async move {
            ma.wait_connected().await;
            for _ in 0..60 {
                if let Ok(Some(q)) = ma.player_for_output(&id).await {
                    println!("MA queue id: {q}");
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
    }
    while let Some(ev) = events.recv().await {
        println!("{ev:?}");
    }
    Ok(())
}
