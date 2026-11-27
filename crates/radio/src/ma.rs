//! Presets through Music Assistant: find a station by name, play a preset on a queue.

use log::{debug, info};
use ma_client::{Client, QueueOption, Radio};
use serde_json::{json, Value};

use crate::Preset;

/// How a preset's name was matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub uri: String,
    /// The station's name in MA (may differ in case or detail from the preset's).
    pub name: String,
    /// Found in the MA library (true) or by a search of the radio providers (false; it has
    /// been added to the library since).
    pub from_library: bool,
}

#[derive(Debug)]
pub enum Error {
    Ma(ma_client::Error),
    NotFound(String),
}

impl std::error::Error for Error {}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Ma(e) => write!(f, "music assistant: {e}"),
            Error::NotFound(n) => write!(f, "no radio station called {n:?} in Music Assistant"),
        }
    }
}

impl From<ma_client::Error> for Error {
    fn from(e: ma_client::Error) -> Self {
        Error::Ma(e)
    }
}

/// The best match for `name`: an exact (case-insensitive) name first, then one that starts
/// with it, then the first result.
pub fn best_match<'a>(name: &str, stations: &'a [Radio]) -> Option<&'a Radio> {
    let want = name.trim().to_lowercase();
    let lower = |r: &Radio| r.name.trim().to_lowercase();
    stations
        .iter()
        .find(|r| lower(r) == want)
        .or_else(|| stations.iter().find(|r| lower(r).starts_with(&want)))
        .or_else(|| stations.first())
        .filter(|r| !r.uri.is_empty())
}

/// Find the MA station for a preset that has only a name: the library first, then every radio
/// provider (Radio Browser, TuneIn, ...). A station found by search is added to the library,
/// so it is mirrored like the rest and keeps a stable `library://radio/<id>` URI.
pub async fn resolve(client: &Client, name: &str) -> Result<Resolved, Error> {
    let lib: Vec<Radio> = client.command("music/radios/library_items", json!({"search": name, "limit": 25, "offset": 0})).await?;
    if let Some(r) = best_match(name, &lib) {
        debug!("N.RADIO: {name:?} is library station {:?} ({})", r.name, r.uri);
        return Ok(Resolved { uri: r.uri.clone(), name: r.name.clone(), from_library: true });
    }
    let found: Value =
        client.command_raw("music/search", json!({"search_query": name, "media_types": ["radio"], "limit": 10})).await?;
    let stations: Vec<Radio> = found
        .get("radio")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    let r = best_match(name, &stations).ok_or_else(|| Error::NotFound(name.to_string()))?;
    info!("N.RADIO: {name:?} found as {:?} ({}); adding it to the library", r.name, r.uri);
    let added: Radio = client.command("music/library/add_item", json!({"item": r.uri})).await?;
    let uri = if added.uri.is_empty() { r.uri.clone() } else { added.uri };
    Ok(Resolved { uri, name: r.name.clone(), from_library: false })
}

/// Play a preset on a queue (the BM5's player id), replacing what was queued.
pub async fn play(client: &Client, queue_id: &str, preset: &Preset) -> Result<(), Error> {
    client.play_media(queue_id, &[&preset.uri], QueueOption::Replace).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio(name: &str, uri: &str) -> Radio {
        Radio { name: name.into(), uri: uri.into(), ..Default::default() }
    }

    #[test]
    fn exact_then_prefix_then_first() {
        let s = [radio("Deutschlandfunk Kultur", "a"), radio("deutschlandfunk", "b"), radio("DLF Nova", "c")];
        assert_eq!(best_match("Deutschlandfunk", &s).unwrap().uri, "b");
        let s = [radio("NRK P1 Oslo", "a"), radio("Radio NRK P1", "b")];
        assert_eq!(best_match("NRK P1", &s).unwrap().uri, "a");
        let s = [radio("Something else", "x")];
        assert_eq!(best_match("NRK P1", &s).unwrap().uri, "x");
        assert!(best_match("NRK P1", &[]).is_none());
        assert!(best_match("x", &[radio("x", "")]).is_none(), "no URI, nothing to play");
    }
}
