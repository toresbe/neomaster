//! Mirror the live MA library into a data directory, bake the atlas, and
//! print what the UI would see. Keeps following events for `[seconds]`.
//!
//!     MA_URL=http://192.168.2.238:8095 MA_TOKEN=... \
//!       cargo run --release --example mirror -- <data_dir> [seconds] [contact_sheet.ppm]

use std::time::Duration;

use library::sync::{self, SyncConfig, Update};
use library::{Atlas, Kind, PixelFormat, Store};
use ma_client::{Client, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or("/tmp/neomaster-library".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let sheet = args.next();
    let url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
    let token = std::env::var("MA_TOKEN")?;

    let (client, events) = Client::connect(Config::new(url, token));
    let cfg = SyncConfig::new(&dir);
    let atlas_cfg = cfg.atlas.clone();
    let (handle, mut updates) = sync::spawn(client.clone(), cfg.clone())?;
    sync::forward_events(handle, events, None);

    // Wait for the first full cycle: sync, covers, atlas, playlists.
    let t0 = std::time::Instant::now();
    let idle_deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        tokio::select! {
            u = updates.recv() => {
                let Some(u) = u else { break };
                match &u {
                    Update::LibraryChanged(_) | Update::PlaylistTracksChanged(_) => {}
                    other => println!("[{:>6} ms] {other:?}", t0.elapsed().as_millis()),
                }
                if matches!(u, Update::Idle) { break; }
            }
            _ = tokio::time::sleep_until(idle_deadline) => break,
        }
    }

    let store = Store::open(cfg.db_path())?;
    for k in Kind::ALL {
        println!("{k:?}: {}", store.count(k)?);
    }
    let letters = store.letter_index(Kind::Artist)?;
    println!("artist letters: {}", letters.iter().map(|(l, i)| format!("{l}@{i}")).collect::<Vec<_>>().join(" "));
    for r in store.list(Kind::Album, 0, 5)? {
        println!("album {:>4} {:<40} {:<25} slot={:?} rgb={:06x?}", r.item_id, r.name, r.subtitle, r.cover_slot, r.cover_rgb);
    }
    // Time the queries the UI makes on every wheel step.
    let t = std::time::Instant::now();
    let n = store.count(Kind::Track)?;
    for off in (0..n).step_by(97) {
        store.list(Kind::Track, off, 12)?;
    }
    println!("track window queries: {:.3} ms each", t.elapsed().as_secs_f64() * 1000.0 / (n / 97 + 1) as f64);
    if let Some(a) = store.list(Kind::Artist, 0, 2000)?.into_iter().find(|a| !store.artist_albums(&a.item_id).unwrap().is_empty())
    {
        let albums = store.artist_albums(&a.item_id)?;
        println!(
            "artist {} -> {} albums; first has {} library tracks",
            a.name,
            albums.len(),
            store.album_tracks(&albums[0].item_id)?.len()
        );
    }
    if let Some(p) = store.list(Kind::Playlist, 0, 200)?.into_iter().find(|p| store.has_playlist_tracks(&p.item_id).unwrap()) {
        println!("playlist {} -> {} tracks", p.name, store.playlist_tracks(&p.item_id)?.len());
    }
    let gen = store.atlas_generation()?;
    for &size in &atlas_cfg.sizes {
        let a = Atlas::open_for(&atlas_cfg, size)?;
        println!(
            "atlas {size:>3}px gen {} slots {} ({} KiB)",
            a.generation,
            a.slots,
            std::fs::metadata(atlas_cfg.path(size))?.len() / 1024
        );
        assert_eq!(a.generation, gen);
    }

    if let Some(path) = sheet {
        // Contact sheet of the first 64 album covers at 96 px, for eyeballing.
        let a = Atlas::open_for(&atlas_cfg, 96)?;
        let albums = store.list(Kind::Album, 0, 64)?;
        let (cols, s) = (8usize, 96usize);
        let rows = albums.len().div_ceil(cols);
        let mut img = vec![0u8; cols * s * rows * s * 3];
        for (n, r) in albums.iter().enumerate() {
            let Some(px) = r.cover_slot.and_then(|i| a.slot(i)) else {
                continue;
            };
            for y in 0..s {
                for x in 0..s {
                    let o = ((n / cols * s + y) * cols * s + (n % cols) * s + x) * 3;
                    let (r8, g8, b8) = match a.format {
                        PixelFormat::Rgb565 => {
                            let v = u16::from_le_bytes([px[(y * s + x) * 2], px[(y * s + x) * 2 + 1]]);
                            (((v >> 11) as u8) << 3, (((v >> 5) & 63) as u8) << 2, ((v & 31) as u8) << 3)
                        }
                        PixelFormat::Xrgb8888 => {
                            let i = (y * s + x) * 4;
                            (px[i + 2], px[i + 1], px[i])
                        }
                    };
                    img[o..o + 3].copy_from_slice(&[r8, g8, b8]);
                }
            }
        }
        let mut out = format!("P6\n{} {}\n255\n", cols * s, rows * s).into_bytes();
        out.extend(img);
        std::fs::write(&path, out)?;
        println!("wrote {path}");
    }

    if secs > 0 {
        println!("following events for {secs} s");
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            tokio::select! {
                u = updates.recv() => match u { Some(u) => println!("{u:?}"), None => break },
                _ = tokio::time::sleep_until(end) => break,
            }
        }
    }
    Ok(())
}
