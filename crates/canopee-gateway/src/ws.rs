//! A minimal RFC 6455 WebSocket *server* implementation, hand-rolled the
//! same way `canopee-cli`'s static HTTP server is (no external web
//! framework). Supports what the gateway needs: the opening handshake,
//! masked client frames, text/binary data frames, ping/pong keep-alive, and
//! connection close. Fragmented messages are rejected (browsers shouldn't
//! send them for these small messages).

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

use crate::SessionToken;
use crate::protocol::{GatewayCommand, GatewayEvent};

/// The magic GUID every RFC 6455 accept key is derived from.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Upper bound on a single message/frame payload the gateway will buffer.
const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
/// How long an opening handshake may take before the connection is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

fn protocol_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::anyhow!("websocket protocol error: {}", message.into())
}

/// One parsed incoming frame.
enum Frame {
    /// A text or binary data frame (the payload, unmasked).
    Data(Vec<u8>),
    Ping(Vec<u8>),
    #[allow(dead_code)]
    Pong(Vec<u8>),
    Close(Option<u16>),
}

/// A single authenticated WebSocket connection to the gateway.
pub struct WebSocket {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

/// The result of an opening exchange on a raw connection: either an
/// authenticated WebSocket, or a plain HTTP response already served (e.g. the
/// demo page) after which the connection is finished.
pub enum Accepted {
    Socket(WebSocket),
    Served,
}

impl WebSocket {
    /// Performs the RFC 6455 opening handshake on `stream`, enforcing the
    /// gateway's session token (in the request query string) and rejecting
    /// cross-origin handshakes. A plain `GET /` (no WebSocket upgrade) serves
    /// the gateway's demo page instead, so `http://127.0.0.1:PORT/` opens
    /// straight in a browser.
    pub async fn accept(stream: TcpStream, token: &SessionToken) -> anyhow::Result<Accepted> {
        let (read_half, write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;

        let request = read_handshake(&mut reader).await?;

        if !request.upgrade_websocket {
            if request.method == "GET" && request.path == "/" {
                let page = crate::demo::demo_page(token.token_hex());
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {}",
                    page.len(),
                    page
                );
                let _ = writer.write_all(response.as_bytes()).await;
                let _ = writer.flush().await;
                return Ok(Accepted::Served);
            }
            let _ = writer
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Err(anyhow::anyhow!("not a websocket upgrade request"));
        }

        if let Err(e) = validate_handshake(&request, token).await {
            let _ = writer
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Err(e);
        }

        let accept = accept_key(&request);
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\
             \r\n"
        );
        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;

        Ok(Accepted::Socket(WebSocket { reader, writer }))
    }

    /// Reads the next command from the browser. Returns `Ok(None)` when the
    /// connection is closed (or a close frame was received). Ping frames are
    /// answered automatically.
    pub async fn read_command(&mut self) -> anyhow::Result<Option<GatewayCommand>> {
        loop {
            match self.read_frame().await? {
                Some(Frame::Data(bytes)) => {
                    let command = serde_json::from_slice(&bytes)
                        .map_err(|e| anyhow::anyhow!("invalid gateway command frame: {e}"))?;
                    return Ok(Some(command));
                }
                Some(Frame::Ping(payload)) => {
                    self.write_frame(OP_PONG, &payload).await?;
                }
                Some(Frame::Pong(_)) => continue,
                Some(Frame::Close(reason)) => {
                    self.write_close(reason).await?;
                    return Ok(None);
                }
                None => return Ok(None),
            }
        }
    }

    /// Serializes `event` to JSON and sends it as a text frame.
    pub async fn reply(&mut self, event: &GatewayEvent) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(event)?;
        self.write_frame(OP_TEXT, &bytes).await
    }

    async fn read_byte(&mut self) -> anyhow::Result<Option<u8>> {
        let mut byte = [0u8; 1];
        match self.reader.read(&mut byte).await? {
            0 => Ok(None),
            _ => Ok(Some(byte[0])),
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        self.reader.read_exact(buf).await.map_err(|e| {
            anyhow::anyhow!("unexpected end of websocket stream while reading frame: {e}")
        })?;
        Ok(())
    }

    async fn read_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let Some(b0) = self.read_byte().await? else {
            return Ok(None);
        };
        let Some(b1) = self.read_byte().await? else {
            return Err(protocol_error("truncated frame header"));
        };

        let fin = b0 & 0x80 != 0;
        let rsv = (b0 >> 4) & 0x7;
        let opcode = b0 & 0x0f;
        if rsv != 0 {
            return Err(protocol_error("non-zero reserved bits are not supported"));
        }
        let masked = b1 & 0x80 != 0;
        let mut length = (b1 & 0x7f) as u64;
        if length == 126 {
            let mut bytes = [0u8; 2];
            self.read_exact(&mut bytes).await?;
            length = u16::from_be_bytes(bytes) as u64;
        } else if length == 127 {
            let mut bytes = [0u8; 8];
            self.read_exact(&mut bytes).await?;
            length = u64::from_be_bytes(bytes);
        }
        if length > MAX_MESSAGE_BYTES {
            return Err(protocol_error(format!(
                "frame payload of {length} bytes exceeds the {MAX_MESSAGE_BYTES} byte limit"
            )));
        }

        let mut mask = [0u8; 4];
        if masked {
            self.read_exact(&mut mask).await?;
        }
        let mut payload = vec![0u8; length as usize];
        self.read_exact(&mut payload).await?;
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i & 3];
            }
        }

        match (opcode, fin) {
            (OP_TEXT, true) | (OP_BINARY, true) => Ok(Some(Frame::Data(payload))),
            (OP_TEXT | OP_BINARY, false) | (OP_CONTINUATION, _) => {
                Err(protocol_error("fragmented messages are not supported"))
            }
            (OP_PING, _) => Ok(Some(Frame::Ping(payload))),
            (OP_PONG, _) => Ok(Some(Frame::Pong(payload))),
            (OP_CLOSE, true) => {
                let reason = if payload.len() >= 2 {
                    Some(u16::from_be_bytes([payload[0], payload[1]]))
                } else {
                    None
                };
                Ok(Some(Frame::Close(reason)))
            }
            (OP_CLOSE, false) => Err(protocol_error("fragmented close frame")),
            (other, _) => Err(protocol_error(format!("unknown opcode {other:#x}"))),
        }
    }

    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> anyhow::Result<()> {
        let mut header = Vec::with_capacity(10);
        header.push(0x80 | opcode);
        let length = payload.len() as u64;
        if length < 126 {
            header.push(length as u8);
        } else if length <= u16::MAX as u64 {
            header.push(126);
            header.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            header.push(127);
            header.extend_from_slice(&length.to_be_bytes());
        }
        self.writer.write_all(&header).await?;
        self.writer.write_all(payload).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn write_close(&mut self, reason: Option<u16>) -> anyhow::Result<()> {
        let payload = match reason {
            Some(code) => code.to_be_bytes().to_vec(),
            None => Vec::new(),
        };
        self.write_frame(OP_CLOSE, &payload).await
    }
}

/// A parsed opening-handshake HTTP request (just enough of it to validate).
struct Handshake {
    method: String,
    path: String,
    upgrade_websocket: bool,
    origin: Option<String>,
    sec_websocket_key: Option<String>,
}

async fn read_handshake(reader: &mut BufReader<OwnedReadHalf>) -> anyhow::Result<Handshake> {
    let line = timeout(HANDSHAKE_TIMEOUT, read_http_line(reader))
        .await
        .map_err(|_| anyhow::anyhow!("handshake timed out"))??;

    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut upgrade_websocket = false;
    let mut origin = None;
    let mut sec_websocket_key = None;

    loop {
        let line = timeout(HANDSHAKE_TIMEOUT, read_http_line(reader))
            .await
            .map_err(|_| anyhow::anyhow!("handshake timed out"))??;
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "upgrade" if value.eq_ignore_ascii_case("websocket") => upgrade_websocket = true,
            "sec-websocket-key" => sec_websocket_key = Some(value.to_string()),
            "origin" => origin = Some(value.to_string()),
            _ => {}
        }
    }

    Ok(Handshake {
        method,
        path,
        upgrade_websocket,
        origin,
        sec_websocket_key,
    })
}

/// Reads one HTTP header line; `Ok("")` means the blank separator line (or
/// the end of the request head). Returns `Err` if the connection ends.
async fn read_http_line(reader: &mut BufReader<OwnedReadHalf>) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte).await? {
            0 => {
                if buf.is_empty() {
                    return Err(anyhow::anyhow!("connection closed during handshake"));
                }
                break;
            }
            _ => {
                if byte[0] == b'\n' {
                    break;
                }
                buf.push(byte[0]);
            }
        }
    }
    let mut line = String::from_utf8_lossy(&buf).to_string();
    if line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

async fn validate_handshake(request: &Handshake, token: &SessionToken) -> anyhow::Result<()> {
    if !request.upgrade_websocket {
        anyhow::bail!("not a websocket upgrade request");
    }
    if request.sec_websocket_key.is_none() {
        anyhow::bail!("missing Sec-WebSocket-Key");
    }
    if !is_loopback_origin(request.origin.as_deref()) {
        anyhow::bail!("origins outside this machine cannot reach the gateway");
    }
    if let Some(candidate) = query_param(&request.path, "token") {
        if token.matches(candidate) {
            return Ok(());
        }
        anyhow::bail!("invalid session token");
    } else {
        anyhow::bail!("missing session token in query string");
    }
}

/// Returns the value of the query parameter `name` from a request path
/// (`/?token=abc...`).
fn query_param<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return Some(value);
        }
    }
    None
}

/// Whether the given `Origin` header value was sent by a page served from a
/// loopback address. Empty/missing origins are allowed (non-browser clients);
/// anything pointing elsewhere is rejected — browsers always include an
/// `Origin` on WebSocket opens, so no site can reach the gateway.
fn is_loopback_origin(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    host.eq_ignore_ascii_case("127.0.0.1") || host.eq_ignore_ascii_case("localhost")
}

/// Computes the `Sec-WebSocket-Accept` value for the client's key (the
/// base64 SHA-1 of `key + GUID`, per RFC 6455 §4.2.2).
fn accept_key(request: &Handshake) -> String {
    let key = request.sec_websocket_key.as_deref().unwrap_or("");
    let mut hasher = Sha1::new();
    hasher.update(key);
    hasher.update(WS_GUID);
    B64.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check_accepts_loopback_and_rejects_foreign() {
        assert!(is_loopback_origin(Some("http://127.0.0.1:8372")));
        assert!(is_loopback_origin(Some("http://localhost:8372")));
        assert!(is_loopback_origin(None));
        assert!(!is_loopback_origin(Some("https://evil.example")));
        assert!(!is_loopback_origin(Some("http://192.168.1.10:9")));
    }

    #[test]
    fn query_param_extracts_token() {
        assert_eq!(query_param("/?token=secret", "token"), Some("secret"));
        assert_eq!(query_param("/?token=a&b=2", "token"), Some("a"));
        assert_eq!(query_param("/", "token"), None);
    }

    #[test]
    fn accept_key_matches_rfc_6455_example() {
        let request = Handshake {
            method: "GET".into(),
            path: "/".into(),
            upgrade_websocket: true,
            origin: None,
            sec_websocket_key: Some("dGhlIHNhbXBsZSBub25jZQ==".into()),
        };
        assert_eq!(accept_key(&request), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
