//! What the panel browses: artists, albums and tracks, each with its Music Assistant URI and
//! cover. neomaster fills it from the local mirror of the MA library (the `library` crate);
//! [`crate::fake`] makes one up for runs without Music Assistant.
//!
//! Indices into the three vectors are what the UI's lists hold, so a [`Library`] is replaced
//! whole when the mirror changes (see [`crate::Ui::set_library`]), never edited in place.

use std::sync::Arc;

use render::Image;

/// Where an item's cover comes from: a slot in the baked cover atlas, and the cover's average
/// colour for while the atlas is not there (or the slot is blank).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cover {
    pub slot: Option<u32>,
    /// 0xRRGGBB.
    pub rgb: Option<u32>,
}

pub struct Track {
    pub title: String,
    /// The artist names as one line.
    pub artist: String,
    /// The track's album, if that album is in the library.
    pub album: Option<usize>,
    pub uri: String,
    pub sort: String,
    pub cover: Cover,
}

pub struct Album {
    pub title: String,
    pub artist: String,
    /// The album's tracks that are in the library, in disc and track order. Playing the album
    /// plays all of it (by its URI), not just these.
    pub tracks: Vec<usize>,
    pub uri: String,
    pub sort: String,
    pub cover: Cover,
}

pub struct Artist {
    pub name: String,
    /// Newest first.
    pub albums: Vec<usize>,
    pub uri: String,
    pub sort: String,
}

/// Cover images by atlas slot.
pub trait CoverSource: Send + Sync {
    /// The cover in `slot`, or `None` if there is no such slot.
    fn cover(&self, slot: u32) -> Option<Arc<Image>>;
}

#[derive(Default)]
pub struct Library {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    /// The cover atlas, once one is baked.
    pub covers: Option<Arc<dyn CoverSource>>,
}

impl Library {
    pub fn empty() -> Library {
        Library::default()
    }

    pub fn album_by_uri(&self, uri: &str) -> Option<usize> {
        self.albums.iter().position(|a| a.uri == uri)
    }

    pub fn track_by_uri(&self, uri: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.uri == uri)
    }

    pub fn artist_by_uri(&self, uri: &str) -> Option<usize> {
        self.artists.iter().position(|a| a.uri == uri)
    }

    /// Album indices in browse order (as ALBUMS and COVERS list them).
    pub fn albums_in_order(&self) -> Vec<usize> {
        let mut a: Vec<usize> = (0..self.albums.len()).collect();
        a.sort_by(|x, y| self.albums[*x].sort.cmp(&self.albums[*y].sort));
        a
    }
}

/// What plays now, as Music Assistant reports it (or as the panel guessed it the moment GO was
/// pressed, until MA says otherwise).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub uri: Option<String>,
    pub album_uri: Option<String>,
    pub cover: Cover,
    /// The large cover for the now-playing view, once fetched.
    pub art: Option<Arc<Image>>,
    /// Playing (not paused or stopped).
    pub playing: bool,
}

/// One entry of the play queue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueEntry {
    pub title: String,
    pub artist: String,
    pub uri: Option<String>,
    pub cover: Cover,
}

/// What GO asks Music Assistant to play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayRequest {
    /// An artist, album, track or playlist URI.
    pub uri: String,
    /// Start at this track of the album `uri` (GO on a track inside an album).
    pub start: Option<String>,
}
