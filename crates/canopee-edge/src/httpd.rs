//! The edge's public HTTP (optionally TLS) front door.
//!
//! Reads browser HTTP requests over TCP (no hyper, mirroring the CLI's
//! hand-rolled static server), resolves `username` from the `Host` header
//! against the registry, forwards the request to the owning publisher over the
//! P2P serve protocol, and relays the response verbatim.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use canopee_network::{NetworkManager, ServeRequest, ServeResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::registry::Registry;

/// A browser connection that goes idle this long is closed.
const KEEP_ALIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// A parsed request from a browser: exactly the subset the edge understands,
/// plus the headers it is allowed to forward to the publisher.
struct Request {
    method: String,
    path: String,
    username: String,
    close_after_response: bool,
    forwarded: Vec<(String, String)>,
}

pub struct Httpd {
    network: Arc<NetworkManager>,
    registry: Registry,
    listener: TcpListener,
    tls: Option<tokio_rustls::TlsAcceptor>,
}

impl Httpd {
    pub async fn new(
        network: NetworkManager,
        registry: Registry,
        port: u16,
        tls: Option<tokio_rustls::TlsAcceptor>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port)).await?;
        Ok(Self {
            network: Arc::new(network),
            registry,
            listener,
            tls,
        })
    }

    /// The port the listener is bound to (differs from the requested port
    /// when port 0 was requested).
    pub fn local_port(&self) -> u16 {
        self.listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or_default()
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let network = self.network;
        let registry = self.registry;

        loop {
            let (stream, addr) = self.listener.accept().await?;
            let network = network.clone();
            let registry = registry.clone();
            let tls = self.tls.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_client(stream, addr, network, registry, tls).await {
                    eprintln!("connection error from {addr}: {e}");
                }
            });
        }
    }
}

/// Builds a `TlsAcceptor` from PEM cert + private key files.
pub fn tls_server_config(
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    let cert_bytes = std::fs::read(cert_path)?;
    let mut cert_reader = std::io::BufReader::new(&cert_bytes[..]);
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader).collect::<Result<_, _>>()?;
    let key_bytes = std::fs::read(key_path)?;
    let mut key_reader = std::io::BufReader::new(&key_bytes[..]);
    let key = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path}"))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

async fn handle_client(
    stream: TcpStream,
    _addr: SocketAddr,
    network: Arc<NetworkManager>,
    registry: Registry,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> anyhow::Result<()> {
    let stream: tokio_util::either::Either<tokio_rustls::server::TlsStream<TcpStream>, TcpStream> =
        match tls {
            Some(acceptor) => tokio_util::either::Either::Left(acceptor.accept(stream).await?),
            None => tokio_util::either::Either::Right(stream),
        };
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    loop {
        let Some(request) = read_request(&mut reader).await? else {
            break;
        };
        let close_after_response = request.close_after_response;
        let response = forward(&network, &registry, &request).await;
        write_response(&mut write_half, &request, &response).await?;
        if close_after_response {
            break;
        }
    }
    Ok(())
}

/// Reads one browser request (request line + headers). Returns `Ok(None)` on
/// EOF or idle timeout, meaning the connection is done.
async fn read_request(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio_util::either::Either<tokio_rustls::server::TlsStream<TcpStream>, TcpStream>>>,
) -> std::io::Result<Option<Request>> {
    loop {
        let mut request_line = String::new();
        let Some(n) = read_line_with_timeout(reader, &mut request_line).await? else {
            return Ok(None);
        };
        if n == 0 {
            return Ok(None);
        }
        // Stray blank lines (keep-alive probes) are skipped.
        if request_line.trim().is_empty() {
            continue;
        }

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("GET").to_string();
        let path = parts
            .next()
            .unwrap_or("/")
            .split('?')
            .next()
            .unwrap_or("/")
            .to_string();
        let http_version = parts.next().unwrap_or("HTTP/1.1");

        let mut close_after_response = http_version == "HTTP/1.0";
        let mut range: Option<String> = None;
        let mut accepts_gzip = false;
        let mut if_none_match: Option<String> = None;
        let mut host: Option<String> = None;

        loop {
            let mut line = String::new();
            let Some(n) = read_line_with_timeout(reader, &mut line).await? else {
                return Ok(None);
            };
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("connection:") {
                close_after_response = value.trim() == "close";
            } else if let Some(value) = lower.strip_prefix("host:") {
                host = Some(value.trim().to_string());
            } else if let Some(value) = lower.strip_prefix("range:") {
                range = Some(value.trim().to_string());
            } else if let Some(value) = lower.strip_prefix("accept-encoding:") {
                accepts_gzip = value.contains("gzip");
            } else if let Some(value) = lower.strip_prefix("if-none-match:") {
                if_none_match = Some(value.trim().to_string());
            }
        }

        let mut forwarded = Vec::new();
        if let Some(range) = range {
            forwarded.push(("range".to_string(), range));
        }
        if accepts_gzip {
            forwarded.push(("accept-encoding".to_string(), "gzip".to_string()));
        }
        if let Some(if_none_match) = if_none_match {
            forwarded.push(("if-none-match".to_string(), if_none_match));
        }

        return Ok(Some(Request {
            method,
            path,
            username: host.as_deref().map(host_subdomain).unwrap_or_default(),
            close_after_response,
            forwarded,
        }));
    }
}

/// The first DNS label of `Host: <user>.<domain>:<port>` — the username the
/// registry is keyed by. A Host without a dot (e.g. `Host: 127.0.0.1:8080`)
/// simply yields a username that no publisher has claimed.
fn host_subdomain(host: &str) -> String {
    let host = host.split(':').next().unwrap_or(host).trim();
    host.split('.')
        .next()
        .unwrap_or(host)
        .to_ascii_lowercase()
}

/// Routes the request to the registered publisher for its `Host` subdomain
/// and awaits their response.
async fn forward(
    network: &NetworkManager,
    registry: &Registry,
    request: &Request,
) -> ServeResponse {
    let peer = registry.resolve(&request.username).await;
    let Some(peer) = peer else {
        return not_found(&format!("no serving node at `{}`", request.username));
    };

    let serve_request = ServeRequest {
        method: request.method.clone(),
        path: request.path.clone(),
        headers: request.forwarded.clone(),
    };
    let timeout = registry.forward_timeout();
    tracing::debug!(
        "forwarding {} {} for `{}` to {peer}",
        request.method,
        request.path,
        request.username
    );
    match tokio::time::timeout(timeout, network.send_serve_request(peer, serve_request)).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => ServeResponse {
            status: "502 Bad Gateway".into(),
            headers: vec![(
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body: format!("upstream error: {e}").into_bytes(),
        },
        Err(_) => ServeResponse {
            status: "504 Gateway Timeout".into(),
            headers: vec![(
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body: b"publisher did not respond in time".to_vec(),
        },
    }
}

fn not_found(message: &str) -> ServeResponse {
    ServeResponse {
        status: "404 Not Found".into(),
        headers: vec![
            ("Content-Type".to_string(), "text/plain; charset=utf-8".to_string()),
            ("Content-Length".to_string(), message.len().to_string()),
        ],
        body: message.as_bytes().to_vec(),
    }
}

/// Relays the publisher's response verbatim, adding only the framing headers
/// the edge owns (`Connection`, plus `Content-Length` when the publisher left
/// it off and the status code needs one).
async fn write_response(
    writer: &mut tokio::io::WriteHalf<tokio_util::either::Either<tokio_rustls::server::TlsStream<TcpStream>, TcpStream>>,
    request: &Request,
    response: &ServeResponse,
) -> anyhow::Result<()> {
    let connection = if request.close_after_response {
        "close".to_string()
    } else {
        "keep-alive".to_string()
    };

    let mut header = format!("HTTP/1.1 {}\r\n", response.status);
    let mut has_content_length = false;
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        header.push_str(&format!("{name}: {value}\r\n"));
    }
    let sends_body = response.status.starts_with("200")
        || response.status.starts_with("206")
        || response.status.starts_with("404")
        || response.status.starts_with("500")
        || response.status.starts_with("502")
        || response.status.starts_with("504");
    if sends_body && !has_content_length {
        header.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    }
    header.push_str(&format!("Connection: {connection}\r\n\r\n"));

    writer.write_all(header.as_bytes()).await?;
    if sends_body && !response.body.is_empty() && request.method != "HEAD" {
        writer.write_all(&response.body).await?;
    }
    writer.flush().await?;
    Ok(())
}

async fn read_line_with_timeout(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio_util::either::Either<tokio_rustls::server::TlsStream<TcpStream>, TcpStream>>>,
    buf: &mut String,
) -> std::io::Result<Option<usize>> {
    match tokio::time::timeout(KEEP_ALIVE_IDLE_TIMEOUT, reader.read_line(buf)).await {
        Ok(result) => result.map(Some),
        Err(_) => Ok(None),
    }
}