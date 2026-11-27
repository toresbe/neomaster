//! Playback commands against one MA queue.
//!
//!     ma_play <queue_id> now                 # now playing
//!     ma_play <queue_id> queue               # list the queue
//!     ma_play <queue_id> play <uri>...       # replace queue and play
//!     ma_play <queue_id> add <uri>...        # append to queue
//!     ma_play <queue_id> next|previous|pause|play_pause|stop
//!     ma_play <queue_id> delete <queue_item_id>

use ma_client::{Client, Config, QueueOption};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
    let (client, _events) = Client::connect(Config::new(url, std::env::var("MA_TOKEN")?));
    client.wait_connected().await;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (q, cmd, rest) = (args[0].as_str(), args.get(1).map(String::as_str).unwrap_or("now"), &args[2.min(args.len())..]);
    let uris: Vec<&str> = rest.iter().map(String::as_str).collect();
    match cmd {
        "now" => {
            let np = client.now_playing(q).await?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs_f64();
            println!("{np:#?}\nposition now: {:?}", np.position_at(now));
        }
        "queue" => {
            for (i, it) in client.queue_items(q, 0, 500).await?.iter().enumerate() {
                println!("{i:>3} {} {}", it.queue_item_id, it.name);
            }
        }
        "play" => client.play_media(q, &uris, QueueOption::Play).await?,
        "add" => client.play_media(q, &uris, QueueOption::Add).await?,
        "delete" => client.delete_queue_item(q, uris[0]).await?,
        "next" => client.next(q).await?,
        "previous" => client.previous(q).await?,
        "pause" => client.pause(q).await?,
        "play_pause" => client.play_pause(q).await?,
        "stop" => client.stop(q).await?,
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}
