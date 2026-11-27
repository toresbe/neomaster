//! Turn the stock BM5's N.RADIO database into neomaster's preset file.
//!
//!     cargo run -p radio --features stock --example import_stock -- nradio.db [radio-presets.toml]
//!
//! `nradio.db` is `E:\Cache\Data\nradio.db` from the BM5's data partition; copy it off a
//! read-only mount (`mount -t ntfs-3g -o ro /dev/sda3 /mnt/e`). Without an output path the
//! TOML goes to stdout. An existing output file keeps its `last` and any presets the stock
//! list does not have; stock presets replace ones with the same number.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let db = PathBuf::from(args.next().ok_or("usage: import_stock nradio.db [radio-presets.toml]")?);
    let out = args.next().map(PathBuf::from);
    let found = radio::stock::import(&db)?;
    for (list, name, url) in &found.colour_items {
        eprintln!("not imported (colour list {list}, no number): {name} <{url}>");
    }
    let mut presets = out.as_deref().map(radio::store::load).unwrap_or_default();
    for p in found.presets.iter() {
        eprintln!("preset {:2}: {} <{}>", p.number, p.name, p.uri);
        presets.set(p.clone());
    }
    match out {
        Some(path) => {
            radio::store::save(&path, &presets)?;
            eprintln!("wrote {} preset(s) to {}", presets.presets.len(), path.display());
        }
        None => print!("{}", toml::to_string(&presets)?),
    }
    Ok(())
}
