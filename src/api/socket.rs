//! `WS /api/ws`, on a thread of its own.
//!
//! The socket is a doorbell, not a feed: every event says only "task X
//! moved", "someone commented on Y", "your planner changed", and the client
//! refetches over HTTP. So this side is small — connect with the profile's
//! socket token, say what's on screen (`{"watch": {"kind", "id"}}`), forward
//! whatever arrives, and keep reconnecting with backoff.
//!
//! Blocking tungstenite on a plain thread, like every other call here runs
//! blocking `ureq` on the background executor. A short read timeout lets the
//! one thread both wait for frames and notice a new watch or being dropped.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest as _;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{HandshakeError, Message, WebSocket};

/// How often the thread looks up from waiting for a frame.
const POLL: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// A laptop waking up holds a socket that died hours ago and looks fine;
/// pinging is how that is found out.
const PING_EVERY: Duration = Duration::from_secs(30);
const SILENCE_LIMIT: Duration = Duration::from_secs(75);
const MAX_BACKOFF_SECS: u64 = 30;

/// What the socket thread reports.
#[derive(Debug)]
pub enum Signal {
    Connected,
    Disconnected,
    /// The token was refused (revoked, or the account is gone). The thread
    /// has stopped; retrying with the same token can't help.
    Unauthorized,
    Event(Value),
}

/// `http(s)://host` → `ws(s)://host/api/ws`.
pub fn socket_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| format!("wss://{base}"));
    format!("{rest}/api/ws")
}

/// The handle to one socket. Dropping it stops the thread (within a poll).
pub struct Socket {
    commands: Sender<Option<String>>,
}

impl Socket {
    /// Starts the thread. `deliver` gets every signal and returns `false`
    /// once nobody is listening any more, which also stops the thread.
    pub fn spawn(
        url: String,
        token: String,
        deliver: impl FnMut(Signal) -> bool + Send + 'static,
    ) -> Self {
        let (commands, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ayeaye-socket".into())
            .spawn(move || worker(&url, &token, rx, deliver))
            .expect("could not start the socket thread");
        Self { commands }
    }

    /// What this socket should watch: `"task:<id>"`, `"org:<id>"`. The server
    /// keeps one watch per socket, so this replaces the last one; `None` asks
    /// for nothing new (there is no unwatch).
    pub fn watch(&self, key: Option<String>) {
        let _ = self.commands.send(key);
    }
}

fn worker(
    url: &str,
    token: &str,
    commands: Receiver<Option<String>>,
    mut deliver: impl FnMut(Signal) -> bool,
) {
    let mut watch: Option<String> = None;
    let mut attempt = 0u32;
    loop {
        if !drain(&commands, &mut watch) {
            return;
        }
        match connect(url, token) {
            Ok(ws) => {
                attempt = 0;
                if !deliver(Signal::Connected) {
                    return;
                }
                if !run(ws, &commands, &mut watch, &mut deliver) {
                    return;
                }
                if !deliver(Signal::Disconnected) {
                    return;
                }
            }
            Err(ConnectError::Unauthorized) => {
                deliver(Signal::Unauthorized);
                return;
            }
            Err(ConnectError::Other(e)) => log::debug!("socket: {e}"),
        }
        // 1s, 2s, 4s … 30s, still noticing being dropped meanwhile.
        let delay = Duration::from_secs((1u64 << attempt.min(5)).min(MAX_BACKOFF_SECS));
        attempt += 1;
        let until = Instant::now() + delay;
        while Instant::now() < until {
            std::thread::sleep(POLL);
            if !drain(&commands, &mut watch) {
                return;
            }
        }
    }
}

/// Takes the latest watch; `false` once the handle is gone.
fn drain(commands: &Receiver<Option<String>>, watch: &mut Option<String>) -> bool {
    loop {
        match commands.try_recv() {
            Ok(Some(key)) => *watch = Some(key),
            Ok(None) => {}
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// One connection's life. `false` means stop for good.
fn run(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    commands: &Receiver<Option<String>>,
    watch: &mut Option<String>,
    deliver: &mut impl FnMut(Signal) -> bool,
) -> bool {
    // Said again on every connect: a new socket remembers nothing.
    if let Some(key) = watch.as_deref()
        && send_watch(&mut ws, key).is_err()
    {
        return true;
    }
    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                heard = Instant::now();
                // A malformed frame isn't worth dropping the socket for.
                if let Ok(event) = serde_json::from_str::<Value>(text.as_str())
                    && !deliver(Signal::Event(event))
                {
                    let _ = ws.close(None);
                    return false;
                }
            }
            Ok(Message::Close(_)) => return true,
            Ok(_) => heard = Instant::now(),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => {
                log::info!("socket dropped: {e}");
                return true;
            }
        }
        loop {
            match commands.try_recv() {
                Ok(Some(key)) => {
                    if watch.as_deref() != Some(key.as_str()) && send_watch(&mut ws, &key).is_err()
                    {
                        *watch = Some(key);
                        return true;
                    }
                    *watch = Some(key);
                }
                Ok(None) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return false;
                }
            }
        }
        if pinged.elapsed() >= PING_EVERY {
            pinged = Instant::now();
            if ws.send(Message::Ping(Default::default())).is_err() {
                return true;
            }
        }
        if heard.elapsed() > SILENCE_LIMIT {
            log::info!(
                "socket silent for {}s, reconnecting",
                SILENCE_LIMIT.as_secs()
            );
            return true;
        }
    }
}

fn send_watch(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, key: &str) -> tungstenite::Result<()> {
    let Some((kind, id)) = key.split_once(':') else {
        return Ok(());
    };
    let frame = json!({"watch": {"kind": kind, "id": id}}).to_string();
    ws.send(Message::text(frame))
}

#[derive(Debug)]
pub enum ConnectError {
    Unauthorized,
    Other(String),
}

impl<E: std::fmt::Display> From<E> for ConnectError {
    fn from(e: E) -> Self {
        ConnectError::Other(e.to_string())
    }
}

/// Opens the socket, with the token in an `Authorization` header — never in
/// the query string, where it would end up in access logs.
pub fn connect(
    url: &str,
    token: &str,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, ConnectError> {
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse()?);
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or("no host")?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });

    // `localhost` is ::1 and 127.0.0.1; a server may listen on only one.
    let mut tcp = None;
    let mut last_error = String::from("no address");
    for addr in (host.as_str(), port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    let tcp = tcp.ok_or(last_error)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let handle = tcp.try_clone()?;

    let ws = match tungstenite::client_tls_with_config(request, tcp, None, None) {
        Ok((ws, _)) => ws,
        Err(HandshakeError::Failure(tungstenite::Error::Http(response)))
            if matches!(response.status().as_u16(), 401 | 403) =>
        {
            return Err(ConnectError::Unauthorized);
        }
        Err(HandshakeError::Failure(e)) => return Err(e.into()),
        Err(HandshakeError::Interrupted(_)) => return Err("handshake timed out".into()),
    };
    // From here on, short reads so the thread can look up between frames.
    handle.set_read_timeout(Some(POLL))?;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn socket_urls() {
        assert_eq!(socket_url("http://localhost"), "ws://localhost/api/ws");
        assert_eq!(
            socket_url("https://tasks.example.com/"),
            "wss://tasks.example.com/api/ws"
        );
        assert_eq!(
            socket_url("http://127.0.0.1:8000"),
            "ws://127.0.0.1:8000/api/ws"
        );
    }
}
