//! The debug socket: JSON Lines both ways, on a Unix socket and optionally on TCP.
//!
//! A client gets a `hello` record, then the stream (filtered by its `filter` command), and the
//! answer to each command it sends: `{"kind":"ok"}` or `{"kind":"error","error":".."}`.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use log::{info, warn};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

use super::inject::Command;
use super::{Line, Tap};
use crate::core::Event;

/// How the debug connection is set up. All off by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Serve the Unix socket here.
    pub socket: Option<PathBuf>,
    /// Also serve TCP here (e.g. `0.0.0.0:7155`, for the LAN).
    pub tcp: Option<SocketAddr>,
    /// Allow injection over TCP (always allowed on the Unix socket).
    pub tcp_inject: bool,
    /// Record the whole stream to this JSON Lines file from start-up.
    pub record: Option<PathBuf>,
}

impl Config {
    pub fn enabled(&self) -> bool {
        self.socket.is_some() || self.tcp.is_some() || self.record.is_some()
    }

    /// `NEOMASTER_DEBUG=1` (socket at [`super::default_socket`]), `NEOMASTER_DEBUG_SOCKET`,
    /// `NEOMASTER_DEBUG_TCP` (address, or just a port for all interfaces),
    /// `NEOMASTER_DEBUG_TCP_INJECT=1`, `NEOMASTER_DEBUG_RECORD`.
    pub fn from_env() -> Result<Config, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let truthy = |k: &str| var(k).is_some_and(|v| !matches!(v.as_str(), "0" | "no" | "false" | "off"));
        let mut c = Config::default();
        if truthy("NEOMASTER_DEBUG") || var("NEOMASTER_DEBUG_SOCKET").is_some() {
            c.socket = Some(super::default_socket());
        }
        if let Some(a) = var("NEOMASTER_DEBUG_TCP") {
            c.tcp = Some(parse_tcp(&a)?);
        }
        c.tcp_inject = truthy("NEOMASTER_DEBUG_TCP_INJECT");
        c.record = var("NEOMASTER_DEBUG_RECORD").map(PathBuf::from);
        Ok(c)
    }
}

/// `0.0.0.0:7155`, `192.168.2.75:7155`, `:7155` or `7155` (all interfaces).
pub fn parse_tcp(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    let port_only = s.strip_prefix(':').unwrap_or(s);
    if let Ok(port) = port_only.parse::<u16>() {
        return Ok(SocketAddr::from(([0, 0, 0, 0], port)));
    }
    s.parse().map_err(|_| format!("{s:?} is not an address:port"))
}

/// Start the debug connection: make the tap, install it globally (so the PC2 actor, the logger
/// and cores built from now on use it), start the recorder and the listeners. Returns the tap,
/// or [`Tap::off`] if `cfg` enables nothing.
pub fn start(
    rt: &tokio::runtime::Runtime,
    cfg: &Config,
    events: mpsc::UnboundedSender<Event>,
    hello: Value,
) -> std::io::Result<Tap> {
    if !cfg.enabled() {
        return Ok(Tap::off());
    }
    let tap = Tap::new();
    tap.install_global();
    let _guard = rt.enter();
    let hello = with_protocol(hello);
    if let Some(path) = &cfg.record {
        super::record::start(&tap, path, hello.clone())?;
        info!("Debug: recording to {}", path.display());
    }
    if let Some(path) = &cfg.socket {
        let listener = bind_unix(path)?;
        info!("Debug: socket {}", path.display());
        let (tap, events, hello) = (tap.clone(), events.clone(), hello.clone());
        rt.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let (r, w) = stream.into_split();
                        tokio::spawn(serve(r, w, tap.clone(), events.clone(), hello.clone(), true));
                    }
                    Err(e) => {
                        warn!("Debug socket: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }
    if let Some(addr) = cfg.tcp {
        let std_listener = std::net::TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let inject = cfg.tcp_inject;
        info!("Debug: TCP {addr} ({})", if inject { "injection allowed" } else { "read-only" });
        let (tap, events, hello) = (tap.clone(), events.clone(), hello.clone());
        rt.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        info!("Debug: TCP client {peer}");
                        let _ = stream.set_nodelay(true);
                        let (r, w) = stream.into_split();
                        tokio::spawn(serve(r, w, tap.clone(), events.clone(), hello.clone(), inject));
                    }
                    Err(e) => {
                        warn!("Debug TCP: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }
    Ok(tap)
}

fn with_protocol(hello: Value) -> Value {
    let mut v = json!({ "protocol": super::PROTOCOL_VERSION, "version": env!("CARGO_PKG_VERSION") });
    if let (Value::Object(m), Value::Object(h)) = (&mut v, hello) {
        m.extend(h);
    }
    v
}

/// Bind the Unix socket, replacing a stale one from an earlier run (but not a live one).
pub fn bind_unix(path: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{} is in use by another neomaster", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    tokio::net::UnixListener::bind(path)
}

/// Which records a client wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Everything but the raw USB packets.
    Default,
    All,
    Kinds(HashSet<String>),
}

impl Filter {
    pub fn from_kinds(kinds: Option<Vec<String>>) -> Filter {
        match kinds {
            None => Filter::Default,
            Some(k) if k.iter().any(|k| k == "*") => Filter::Default,
            Some(k) if k.iter().any(|k| k == "all") => Filter::All,
            Some(k) => Filter::Kinds(k.into_iter().collect()),
        }
    }

    pub fn wants(&self, kind: &str) -> bool {
        match self {
            Filter::Default => !kind.starts_with("usb_"),
            Filter::All => true,
            Filter::Kinds(k) => k.contains(kind),
        }
    }
}

/// Serve one client until it hangs up.
pub async fn serve<R, W>(r: R, mut w: W, tap: Tap, events: mpsc::UnboundedSender<Event>, hello: Value, inject: bool)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let Some(mut stream) = tap.subscribe() else { return };
    let mut hello = hello;
    if let Value::Object(m) = &mut hello {
        m.insert("inject".into(), json!(inject));
    }
    let mut filter = Filter::Default;
    let mut lines = BufReader::new(r).lines();
    if send(&mut w, &tap.line("hello", hello)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            rec = stream.recv() => match rec {
                Ok(line) => {
                    if filter.wants(line.kind) && send(&mut w, &line).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    if send(&mut w, &tap.line("lagged", json!({ "missed": n }))).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { return };
                if line.trim().is_empty() {
                    continue;
                }
                let reply = match command(&line, &tap, &events, inject, &mut filter) {
                    Ok(()) => tap.line("ok", json!({})),
                    Err(e) => tap.line("error", json!({ "error": e, "line": line })),
                };
                if send(&mut w, &reply).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn command(
    line: &str,
    tap: &Tap,
    events: &mpsc::UnboundedSender<Event>,
    inject: bool,
    filter: &mut Filter,
) -> Result<(), String> {
    let cmd: Command = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if cmd.injects() && !inject {
        return Err("injection is off on this connection (start neomaster with --debug-tcp-inject)".into());
    }
    match &cmd {
        Command::Filter { kinds } => *filter = Filter::from_kinds(kinds.clone()),
        Command::Mark { text } => tap.emit("mark", || json!({ "text": text })),
        Command::Ping => {}
        _ => {}
    }
    if let Some(i) = cmd.to_inject()? {
        events.send(Event::Debug(i)).map_err(|_| "the core has stopped".to_string())?;
    }
    Ok(())
}

async fn send<W: AsyncWrite + Unpin>(w: &mut W, line: &Line) -> std::io::Result<()> {
    w.write_all(line.json.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::Inject;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn tcp_addresses() {
        assert_eq!(parse_tcp("7155").unwrap(), "0.0.0.0:7155".parse().unwrap());
        assert_eq!(parse_tcp(":7155").unwrap(), "0.0.0.0:7155".parse().unwrap());
        assert_eq!(parse_tcp("192.168.2.75:9000").unwrap(), "192.168.2.75:9000".parse().unwrap());
        assert!(parse_tcp("bm5").is_err());
    }

    #[test]
    fn filters() {
        assert!(Filter::Default.wants("ml_in"));
        assert!(!Filter::Default.wants("usb_in"));
        assert!(Filter::All.wants("usb_in"));
        let f = Filter::from_kinds(Some(vec!["ml_in".into()]));
        assert!(f.wants("ml_in") && !f.wants("ml_out"));
        assert!(!Filter::from_kinds(Some(vec![])).wants("ml_in"));
    }

    async fn read_json(lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>) -> Value {
        let l = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line()).await.unwrap().unwrap().unwrap();
        serde_json::from_str(&l).unwrap()
    }

    /// A client on the Unix socket: hello, the stream with its filter, injection reaching the
    /// core's event channel, errors for bad commands.
    #[tokio::test]
    async fn unix_socket_streams_and_injects() {
        let dir = std::env::temp_dir().join(format!("neomaster-debug-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("debug.sock");
        let tap = Tap::new();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let listener = bind_unix(&path).unwrap();
        // a second server on the same path is refused while the first is alive
        assert!(bind_unix(&path).is_err());
        let server_tap = tap.clone();
        // (the probe above was a connection too: serve every one, as start() does)
        tokio::spawn(async move {
            loop {
                let (s, _) = listener.accept().await.unwrap();
                let (r, w) = s.into_split();
                tokio::spawn(serve(r, w, server_tap.clone(), ev_tx.clone(), json!({ "role": "test" }), true));
            }
        });

        let client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (r, mut w) = client.into_split();
        let mut lines = BufReader::new(r).lines();
        let hello = read_json(&mut lines).await;
        assert_eq!(hello["kind"], "hello");
        assert_eq!(hello["role"], "test");
        assert_eq!(hello["inject"], true);

        w.write_all(b"{\"cmd\":\"filter\",\"kinds\":[\"ml_in\",\"mark\"]}\n").await.unwrap();
        assert_eq!(read_json(&mut lines).await["kind"], "ok");
        tap.emit("ml_out", || json!({}));
        tap.emit("ml_in", || json!({ "n": 1 }));
        assert_eq!(read_json(&mut lines).await["n"], 1);

        w.write_all(b"{\"cmd\":\"beo4\",\"code\":\"standby\",\"held\":true}\n").await.unwrap();
        assert_eq!(read_json(&mut lines).await["kind"], "ok");
        match ev_rx.recv().await.unwrap() {
            Event::Debug(Inject::Beo4 { kind: 0x0f, code: 0x0c }) => {}
            e => panic!("unexpected {e:?}"),
        }

        w.write_all(b"{\"cmd\":\"mark\",\"text\":\"here\"}\n").await.unwrap();
        // the ok, and the mark broadcast to everyone (and so recorded), in either order
        let mut got = [read_json(&mut lines).await, read_json(&mut lines).await];
        got.sort_by_key(|v| v["kind"].as_str().unwrap_or("").to_string());
        assert_eq!((got[0]["kind"].as_str(), got[0]["text"].as_str()), (Some("mark"), Some("here")));
        assert_eq!(got[1]["kind"], "ok");

        w.write_all(b"not json\n").await.unwrap();
        assert_eq!(read_json(&mut lines).await["kind"], "error");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without injection (TCP by default), changes are refused but reading and marks work.
    #[tokio::test]
    async fn read_only_connections_refuse_injection() {
        let tap = Tap::new();
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (client, server) = tokio::io::duplex(4096);
        let (sr, sw) = tokio::io::split(server);
        tokio::spawn(serve(sr, sw, tap.clone(), ev_tx, json!({}), false));
        let (cr, mut cw) = tokio::io::split(client);
        let mut lines = BufReader::new(cr).lines();
        let next = |l: String| serde_json::from_str::<Value>(&l).unwrap();
        assert_eq!(next(lines.next_line().await.unwrap().unwrap())["inject"], false);
        cw.write_all(b"{\"cmd\":\"telegram\",\"dir\":\"out\",\"hex\":\"80 c2 01 0a 00 00 00 11 00 01\"}\n").await.unwrap();
        let reply = next(lines.next_line().await.unwrap().unwrap());
        assert_eq!(reply["kind"], "error");
        assert!(ev_rx.try_recv().is_err());
        cw.write_all(b"{\"cmd\":\"state\"}\n").await.unwrap();
        assert_eq!(next(lines.next_line().await.unwrap().unwrap())["kind"], "ok");
        assert!(matches!(ev_rx.recv().await, Some(Event::Debug(Inject::Snapshot))));
    }
}
