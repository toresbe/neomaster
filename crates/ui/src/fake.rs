//! Made-up library data for runs without Music Assistant (and for tests): about 150 artists
//! with albums and tracks, `fake://` URIs and average colours instead of covers.

pub use crate::library::Library;
use crate::library::{Album, Artist, Cover, Track};

const FIRST: &[&str] = &[
    "Amber", "Brass", "Cedar", "Copper", "Crystal", "Dusk", "Ember", "Foxglove", "Glass", "Harbour", "Indigo", "Juniper",
    "Kestrel", "Lantern", "Marble", "Neon", "Northern", "Opal", "Paper", "Quiet", "Rust", "Saffron", "Silver", "Tidal", "Umber",
    "Velvet", "Willow", "Winter", "Yarrow", "Zephyr",
];
const SECOND: &[&str] = &[
    "Atlas",
    "Avenue",
    "Choir",
    "Circuit",
    "Coast",
    "Collective",
    "Echo",
    "Engine",
    "Fields",
    "Garden",
    "Hearts",
    "Hollow",
    "Lights",
    "Machine",
    "Orchestra",
    "Parade",
    "Pilots",
    "Quartet",
    "Radio",
    "Rivers",
    "Signal",
    "Society",
    "Static",
    "Tigers",
    "Trio",
    "Valley",
    "Waves",
];
const WORDS: &[&str] = &[
    "after", "all", "blue", "broken", "city", "cold", "dance", "dark", "days", "dream", "falling", "fire", "golden", "home",
    "late", "light", "lost", "midnight", "morning", "night", "ocean", "old", "rain", "road", "running", "slow", "song", "stars",
    "summer", "sun", "through", "time", "water", "wild", "years",
];

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

fn title(rng: &mut Lcg, words: usize) -> String {
    let mut s = String::new();
    for i in 0..words {
        let w = WORDS[rng.next(WORDS.len())];
        if i > 0 {
            s.push(' ');
        }
        let mut c = w.chars();
        s.extend(c.next().map(|f| f.to_ascii_uppercase()));
        s.push_str(c.as_str());
    }
    s
}

/// A deterministic library of about 150 artists.
pub fn generate() -> Library {
    let mut rng = Lcg(0x5eed);
    let mut names: Vec<String> = Vec::new();
    for f in FIRST {
        for _ in 0..5 {
            let n = format!("{f} {}", SECOND[rng.next(SECOND.len())]);
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    let mut lib = Library::empty();
    for name in names {
        let artist = lib.artists.len();
        let mut albums = Vec::new();
        for _ in 0..1 + rng.next(4) {
            let album = lib.albums.len();
            let mut tracks = Vec::new();
            for _ in 0..6 + rng.next(7) {
                let i = lib.tracks.len();
                tracks.push(i);
                let words = 1 + rng.next(3);
                let title = title(&mut rng, words);
                lib.tracks.push(Track {
                    sort: title.to_lowercase(),
                    title,
                    artist: name.clone(),
                    album: Some(album),
                    uri: format!("fake://track/{i}"),
                    cover: Cover::default(),
                });
            }
            // drawn after the tracks, as before covers had a type of their own
            let colour = (40 + rng.next(180) as u32, 40 + rng.next(180) as u32, 40 + rng.next(180) as u32);
            let cover = Cover { slot: None, rgb: Some(colour.0 << 16 | colour.1 << 8 | colour.2) };
            for t in &tracks {
                lib.tracks[*t].cover = cover;
            }
            let words = 1 + rng.next(3);
            let title = title(&mut rng, words);
            lib.albums.push(Album {
                sort: title.to_lowercase(),
                title,
                artist: name.clone(),
                tracks,
                uri: format!("fake://album/{album}"),
                cover,
            });
            albums.push(album);
        }
        lib.artists.push(Artist { sort: name.to_lowercase(), uri: format!("fake://artist/{artist}"), name, albums });
    }
    lib
}

impl Library {
    /// The made-up library (see [`generate`]).
    pub fn generate() -> Library {
        generate()
    }
}
