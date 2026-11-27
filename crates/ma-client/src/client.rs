use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{sleep, timeout, Instant};
use tokio_tungstenite::tungstenite::Message;

use crate::events::Event;
use crate::models::ServerInfo;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not connected to Music Assistant")]
    Disconnected,
    #[error("command timed out")]
    Timeout,
    #[error("Music Assistant error {code}: {details}")]
    Server { code: i64, details: String },
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("websocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("protocol: {0}")]
    Protocol(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Connection settings.
#[derive(Clone)]
pub struct Config {
    /// Base URL of the MA server, e.g. `http://192.168.2.238:8095`.
    /// `ws://`/`wss://` URLs and bare `host:port` are accepted too.
    pub url: String,
    /// Long-lived MA access token.
    pub token: String,
    /// If set, refuse to authenticate against a server with another id
    /// (MA's advice for clients that store tokens).
    pub expect_server_id: Option<String>,
    /// Default deadline for [`Client::command`].
    pub command_timeout: Duration,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("expect_server_id", &self.expect_server_id)
            .finish()
    }
}

impl Config {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Config { url: url.into(), token: token.into(), expect_server_id: None, command_timeout: Duration::from_secs(60) }
    }

    /// `ws://host:port/ws` for the configured URL.
    pub fn ws_url(&self) -> String {
        let u = self.url.trim_end_matches('/');
        let (scheme, rest) = match u.split_once("://") {
            Some(("https", r)) | Some(("wss", r)) => ("wss", r),
            Some((_, r)) => ("ws", r),
            None => ("ws", u),
        };
        let rest = rest.strip_suffix("/ws").unwrap_or(rest);
        format!("{scheme}://{rest}/ws")
    }

    /// `http://host:port` for the configured URL (for `/imageproxy`).
    pub fn http_url(&self) -> String {
        let ws = self.ws_url();
        let base = ws.strip_suffix("/ws").unwrap_or(&ws);
        if let Some(r) = base.strip_prefix("wss://") {
            format!("https://{r}")
        } else {
            format!("http://{}", base.strip_prefix("ws://").unwrap_or(base))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnState {
    Connecting,
    Connected(ServerInfo),
    Disconnected,
}

struct Request {
    command: String,
    args: Value,
    reply: oneshot::Sender<Result<Value>>,
}

/// Handle to the MA connection. Cheap to clone; all clones share one socket.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Request>,
    state: watch::Receiver<ConnState>,
    config: std::sync::Arc<Config>,
}

impl Client {
    /// Start the connection task. It connects, authenticates and reconnects
    /// on its own for as long as any `Client` clone or the event receiver is
    /// alive. Must be called inside a tokio runtime.
    pub fn connect(config: Config) -> (Client, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (st_tx, st_rx) = watch::channel(ConnState::Connecting);
        let config = std::sync::Arc::new(config);
        tokio::spawn(supervise((*config).clone(), rx, ev_tx, st_tx));
        (Client { tx, state: st_rx, config }, ev_rx)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn state(&self) -> ConnState {
        self.state.borrow().clone()
    }

    /// Wait until the client is connected and authenticated.
    pub async fn wait_connected(&self) -> ServerInfo {
        let mut st = self.state.clone();
        loop {
            if let ConnState::Connected(info) = &*st.borrow_and_update() {
                return info.clone();
            }
            if st.changed().await.is_err() {
                // Supervisor gone; never resolves.
                std::future::pending::<()>().await;
            }
        }
    }

    /// Send a raw command and return the raw JSON result. Fails at once with
    /// [`Error::Disconnected`] while the socket is down, so callers never
    /// queue up stale UI actions.
    pub async fn command_raw(&self, command: &str, args: Value) -> Result<Value> {
        self.command_raw_timeout(command, args, self.config.command_timeout).await
    }

    pub async fn command_raw_timeout(&self, command: &str, args: Value, deadline: Duration) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Request { command: command.to_owned(), args, reply }).map_err(|_| Error::Disconnected)?;
        match timeout(deadline, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(Error::Disconnected),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// Send a command and decode its result.
    pub async fn command<T: DeserializeOwned>(&self, command: &str, args: Value) -> Result<T> {
        let v = self.command_raw(command, args).await?;
        Ok(serde_json::from_value(v)?)
    }
}

async fn supervise(
    config: Config,
    mut rx: mpsc::UnboundedReceiver<Request>,
    ev_tx: mpsc::UnboundedSender<Event>,
    st_tx: watch::Sender<ConnState>,
) {
    let mut backoff = Duration::from_millis(500);
    loop {
        if ev_tx.is_closed() && rx.is_closed() {
            return;
        }
        let _ = st_tx.send(ConnState::Connecting);
        let started = Instant::now();
        match session(&config, &mut rx, &ev_tx, &st_tx).await {
            Ok(()) => info!("Music Assistant connection closed"),
            Err(e) => warn!("Music Assistant connection: {e}"),
        }
        let was_connected = matches!(&*st_tx.borrow(), ConnState::Connected(_));
        let _ = st_tx.send(ConnState::Disconnected);
        if was_connected {
            let _ = ev_tx.send(Event::Disconnected);
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_millis(500);
        }
        // Fail anything sent while we wait, rather than letting it go stale.
        let wake = Instant::now() + backoff;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(wake) => break,
                r = rx.recv() => match r {
                    Some(req) => { let _ = req.reply.send(Err(Error::Disconnected)); }
                    None => if ev_tx.is_closed() { return } else { sleep(backoff).await; break },
                },
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

struct Pending {
    reply: oneshot::Sender<Result<Value>>,
    partial: Vec<Value>,
}

async fn session(
    config: &Config,
    rx: &mut mpsc::UnboundedReceiver<Request>,
    ev_tx: &mpsc::UnboundedSender<Event>,
    st_tx: &watch::Sender<ConnState>,
) -> Result<()> {
    let url = config.ws_url();
    debug!("connecting to {url}");
    let (mut ws, _) =
        timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(&url)).await.map_err(|_| Error::Timeout)??;

    // 1. Server sends its ServerInfoMessage first.
    let info: ServerInfo = loop {
        match timeout(Duration::from_secs(10), ws.next()).await.map_err(|_| Error::Timeout)? {
            Some(Ok(Message::Text(t))) => break serde_json::from_str(&t)?,
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => return Err(Error::Protocol("closed before server info".into())),
        }
    };
    if let Some(want) = &config.expect_server_id {
        if &info.server_id != want {
            return Err(Error::Auth(format!("unexpected server id {}", info.server_id)));
        }
    }

    // 2. Authenticate.
    let auth = json!({"message_id": "auth", "command": "auth", "args": {"token": config.token}});
    ws.send(Message::Text(auth.to_string().into())).await?;
    loop {
        let msg = match timeout(Duration::from_secs(10), ws.next()).await.map_err(|_| Error::Timeout)? {
            Some(Ok(Message::Text(t))) => serde_json::from_str::<Value>(&t)?,
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => return Err(Error::Auth("closed during auth".into())),
        };
        if msg.get("message_id").and_then(Value::as_str) != Some("auth") {
            continue;
        }
        if let Some(code) = msg.get("error_code") {
            return Err(Error::Auth(format!("{code}: {}", msg.get("details").and_then(Value::as_str).unwrap_or(""))));
        }
        if msg.pointer("/result/authenticated").and_then(Value::as_bool) != Some(true) {
            return Err(Error::Auth("server did not confirm authentication".into()));
        }
        break;
    }
    info!("connected to Music Assistant {} (schema {}) at {url}", info.server_version, info.schema_version);
    let _ = st_tx.send(ConnState::Connected(info.clone()));
    let _ = ev_tx.send(Event::Connected(info));

    // 3. Main loop.
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut next_id: u64 = 1;
    let mut last_rx = Instant::now();
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result = loop {
        tokio::select! {
            req = rx.recv() => {
                let Some(req) = req else { break Ok(()) };
                let id = next_id.to_string();
                next_id += 1;
                let msg = json!({"message_id": id, "command": req.command, "args": req.args});
                debug!("-> {} {}", id, req.command);
                if let Err(e) = ws.send(Message::Text(msg.to_string().into())).await {
                    let _ = req.reply.send(Err(Error::Disconnected));
                    break Err(e.into());
                }
                pending.insert(id, Pending { reply: req.reply, partial: Vec::new() });
            }
            msg = ws.next() => {
                last_rx = Instant::now();
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = match serde_json::from_str(&t) {
                            Ok(v) => v,
                            Err(e) => { warn!("undecodable message: {e}"); continue }
                        };
                        handle_incoming(v, &mut pending, ev_tx);
                    }
                    Some(Ok(Message::Close(_))) | None => break Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(e.into()),
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > Duration::from_secs(60) {
                    break Err(Error::Protocol("no traffic for 60 s".into()));
                }
                if let Err(e) = ws.send(Message::Ping(Vec::new().into())).await {
                    break Err(e.into());
                }
            }
        }
    };
    for (_, p) in pending.drain() {
        let _ = p.reply.send(Err(Error::Disconnected));
    }
    result
}

fn handle_incoming(mut v: Value, pending: &mut HashMap<String, Pending>, ev_tx: &mpsc::UnboundedSender<Event>) {
    if let Some(id) = v.get("message_id") {
        let id = match id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let Some(p) = pending.get_mut(&id) else {
            debug!("reply for unknown message_id {id}");
            return;
        };
        if let Some(code) = v.get("error_code") {
            let p = pending.remove(&id).unwrap();
            let details = v.get("details").and_then(Value::as_str).unwrap_or("").to_owned();
            let _ = p.reply.send(Err(Error::Server { code: code.as_i64().unwrap_or(-1), details }));
            return;
        }
        let result = v.get_mut("result").map(Value::take).unwrap_or(Value::Null);
        let partial = v.get("partial").and_then(Value::as_bool).unwrap_or(false);
        if partial {
            match result {
                Value::Array(items) => p.partial.extend(items),
                other => p.partial.push(other),
            }
            return;
        }
        let p = pending.remove(&id).unwrap();
        let full = if p.partial.is_empty() {
            result
        } else {
            let mut all = p.partial;
            match result {
                Value::Array(items) => all.extend(items),
                Value::Null => {}
                other => all.push(other),
            }
            Value::Array(all)
        };
        let _ = p.reply.send(Ok(full));
    } else if v.get("event").is_some() {
        let _ = ev_tx.send(Event::from_message(v));
    } else {
        debug!("unhandled message: {v}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let c = Config::new("http://192.168.2.238:8095", "t");
        assert_eq!(c.ws_url(), "ws://192.168.2.238:8095/ws");
        assert_eq!(c.http_url(), "http://192.168.2.238:8095");
        let c = Config::new("ma.local:8095/", "t");
        assert_eq!(c.ws_url(), "ws://ma.local:8095/ws");
        let c = Config::new("wss://ma.example/ws", "t");
        assert_eq!(c.http_url(), "https://ma.example");
        assert!(!format!("{c:?}").contains("\"t\""));
    }

    #[test]
    fn partial_results_are_joined() {
        let (ev_tx, _ev) = mpsc::unbounded_channel();
        let (reply, mut rx) = oneshot::channel();
        let mut pending = HashMap::new();
        pending.insert("7".to_string(), Pending { reply, partial: vec![] });
        handle_incoming(json!({"message_id": "7", "result": [1, 2], "partial": true}), &mut pending, &ev_tx);
        assert!(rx.try_recv().is_err());
        handle_incoming(json!({"message_id": "7", "result": [3]}), &mut pending, &ev_tx);
        assert_eq!(rx.try_recv().unwrap().unwrap(), json!([1, 2, 3]));
    }

    #[test]
    fn errors_are_reported() {
        let (ev_tx, _ev) = mpsc::unbounded_channel();
        let (reply, mut rx) = oneshot::channel();
        let mut pending = HashMap::new();
        pending.insert("1".to_string(), Pending { reply, partial: vec![] });
        handle_incoming(json!({"message_id": 1, "error_code": 999, "details": "nope"}), &mut pending, &ev_tx);
        assert!(matches!(rx.try_recv().unwrap(), Err(Error::Server { code: 999, .. })));
    }
}
