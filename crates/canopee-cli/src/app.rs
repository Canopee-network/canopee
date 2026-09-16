use canopee_sdk::CanopeeClient;
use canopee_storage::{AppManifest, Object, ObjectId};
use futures::future::try_join_all;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Opens `url` in the user's default browser, best-effort. Cross-platform:
/// `open` on macOS, `xdg-open` on Linux, `rundll32 url.dll` on Windows.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = tokio::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = tokio::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _ = url; // unsupported platform: nothing to open

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    tokio::spawn(async move { let _ = cmd.spawn(); });
    #[cfg(target_os = "windows")]
    let _ = cmd.spawn();
}

pub async fn publish_directory(
    client: &CanopeeClient,
    directory: &Path,
) -> anyhow::Result<(ObjectId, HashMap<String, ObjectId>)> {
    let mut assets = HashMap::new();
    let mut entrypoint = None;
    let mut entrypoint_html: Option<Vec<u8>> = None;

    for entry in walkdir::WalkDir::new(directory)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let data = tokio::fs::read(path).await.unwrap();
        let relative = path.strip_prefix(directory)?.to_string_lossy().to_string();
        let object_id = client.put_file(&relative, data.clone()).await?;

        println!("{} -> {}", relative, object_id);

        if relative == "index.html" {
            entrypoint = Some(object_id.clone());
            entrypoint_html = Some(data);
        } else {
            assets.insert(format!("/{}", relative), object_id);
        }
    }
    let entrypoint = entrypoint.ok_or(anyhow::anyhow!("portfolio has no index.html"))?;

    // Stretch step: a build with a non-root `base`/`homepage` config
    // references assets under a prefix publish_directory never created
    // (e.g. `/my-app/assets/...` instead of `/assets/...`). Catch that at
    // publish time rather than shipping a site that 404s every asset.
    if let Some(html) = entrypoint_html.as_deref() {
        if let Ok(html) = std::str::from_utf8(html) {
            let missing = unpublished_asset_references(html, &assets);
            if !missing.is_empty() {
                anyhow::bail!(
                    "index.html references paths that were not published (maybe a non-root \
                     build base?): {} — rebuild with `base: '/'` (Vite) or unset `homepage` \
                     (CRA) so assets are referenced from the repository root",
                    missing.join(", ")
                );
            }
        }
    }

    Ok((entrypoint, assets))
}

/// Scans `index.html` for absolute `src="/..."` / `href="/..."` references
/// and returns those with no matching published asset path. A non-root build
/// base makes the HTML point at e.g. `/my-app/assets/x.js` while
/// `publish_directory` only publishes `/assets/x.js`.
fn unpublished_asset_references(
    html: &str,
    assets: &HashMap<String, ObjectId>,
) -> Vec<String> {
    let mut missing = Vec::new();
    for value in find_absolute_urls(html) {
        let key = value
            .split(['?', '#'])
            .next()
            .unwrap_or(value.as_str())
            .to_string();
        if !assets.contains_key(&key) && !missing.contains(&key) {
            missing.push(key);
        }
    }
    missing
}

/// Returns paths from `src="/..."` and `href="/..."` attributes in HTML (a
/// simple substring search covers real bundler output).
fn find_absolute_urls(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    for attr in ["src=", "href="] {
        let mut rest = html;
        while let Some(rel) = rest.find(attr) {
            let after_attr = &rest[rel + attr.len()..];
            // Skip quoted/unquoted: bundlers emit `src="/..."`; handle a
            // leading quote char if present.
            let after_quote = after_attr
                .strip_prefix(['"', '\''])
                .unwrap_or(after_attr);
            let value = after_quote
                .split(|c: char| c == '"' || c == '\'' || c == '>' || c.is_whitespace())
                .next()
                .unwrap_or("");
            if value.starts_with('/') && !value.starts_with("//") {
                found.push(value.to_string());
            }
            rest = after_attr;
        }
    }
    found
}

/// Resolves peers that can serve `id`: returns the explicit `peer` hint
/// first (if given), then every provider found on the DHT for `id`.
async fn resolve_providers(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut providers: Vec<String> = Vec::new();
    if let Some(peer) = peer {
        providers.push(peer.to_string());
    }
    let dht_providers = client.find_providers(id.clone()).await?;
    for p in dht_providers {
        if !providers.contains(&p) {
            providers.push(p);
        }
    }
    Ok(providers)
}

/// Reads an object from local storage, falling back to fetching it from a
/// peer (trying multiple candidates if the first is offline) and importing
/// it locally. After a successful import from a remote peer, announces
/// this node as an additional provider and marks the object as cached.
async fn get_or_fetch(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<Object> {
    if let Ok(object) = client.get(id.clone()).await {
        return Ok(object);
    }

    let providers = resolve_providers(client, id, peer).await?;
    if providers.is_empty() {
        anyhow::bail!("no providers found for {id}; try passing --peer");
    }

    let mut last_err = anyhow::anyhow!("no providers available");
    for peer_id in &providers {
        match client.fetch_object(peer_id, id.clone()).await {
            Ok(bundle) => {
                // Import (the node auto-marks non-owned objects as cached)
                // then announce as a provider — best-effort: a failed announce
                // should not fail the fetch itself.
                client.import(bundle).await?;
                if let Err(e) = client.announce(id.clone()).await {
                    eprintln!("Warning: failed to announce {id} as provider: {e}");
                }
                return client.get(id.clone()).await;
            }
            Err(e) => {
                last_err = e;
                continue;
            }
        }
    }
    Err(last_err)
}

/// Resolves an app manifest and every object it references (entrypoint +
/// assets), fetching whatever isn't already stored locally. Assets are
/// fetched concurrently. Returns the manifest alongside a map of URL path ->
/// file bytes, ready to serve.
pub async fn fetch_app(
    client: &CanopeeClient,
    manifest_id: ObjectId,
    peer: Option<String>,
) -> anyhow::Result<(AppManifest, HashMap<String, Vec<u8>>)> {
    let manifest_object = get_or_fetch(client, &manifest_id, peer.as_deref()).await?;
    let manifest: AppManifest = manifest_object.decode()?;

    // Use the `peer` hint (if given) for every referenced object; otherwise
    // let each object resolve its own provider set from the DHT (a cache node
    // holding one asset may not hold another).
    let entrypoint = get_or_fetch(client, &manifest.entrypoint, peer.as_deref()).await?;
    let mut files = HashMap::new();
    files.insert("/".to_string(), entrypoint.payload.data);

    let asset_fetches = manifest.assets.iter().map(|(path, object_id)| {
        let path = path.clone();
        let peer = peer.clone();
        async move {
            let object = get_or_fetch(client, object_id, peer.as_deref()).await?;
            Ok::<_, anyhow::Error>((path, object.payload.data))
        }
    });
    for (path, data) in try_join_all(asset_fetches).await? {
        files.insert(path, data);
    }

    Ok((manifest, files))
}

/// How long a connection may sit idle (no bytes received) before the server
/// closes it. Keep-alive makes idle sockets a real resource, so this is
/// bounded rather than unbounded.
const KEEP_ALIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bodies smaller than this are not worth gzipping — the gzip header and
/// trailer alone can exceed the savings.
const MIN_COMPRESSIBLE_LEN: usize = 256;

/// Serves `files` (URL path -> bytes) over plain HTTP on `127.0.0.1:port`
/// (pass 0 to let the OS pick a free port) until the process is killed.
/// With `open_browser` set, opens the bound URL in the default browser before
/// serving.
///
/// HTTP/1.1 semantics: keep-alive persistent connections (unless the client
/// asks to close), `Range` single-range requests, `gzip` content encoding for
/// compressible types, `HEAD` with headers but no body, and `ETag` + 304
/// conditional revalidation.
pub async fn serve(files: HashMap<String, Vec<u8>>, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let etags = compute_etags(&files);
    serve_loop(files, etags, port, open_browser).await
}

/// Computes a strong `ETag` for every file: the SHA-256 of its bytes, which
/// is exactly what `ObjectId::from_data` already is — content-addressed
/// identity, reused as HTTP cache identity.
fn compute_etags(files: &HashMap<String, Vec<u8>>) -> HashMap<String, String> {
    files
        .iter()
        .map(|(path, bytes)| (path.clone(), ObjectId::from_data(bytes).0))
        .collect()
}

async fn serve_loop(
    files: HashMap<String, Vec<u8>>,
    etags: HashMap<String, String>,
    port: u16,
    open_browser_after_bind: bool,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let files = Arc::new(files);
    let etags = Arc::new(etags);

    let addr = listener.local_addr()?;
    println!("Serving app at http://{}", addr);
    if open_browser_after_bind {
        open_browser(&format!("http://{addr}"));
    }

    loop {
        let (stream, _) = listener.accept().await?;
        let files = files.clone();
        let etags = etags.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &files, &etags).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

/// Serves requests on a connection until the client closes it, asks to close,
/// or goes idle past `KEEP_ALIVE_IDLE_TIMEOUT`.
async fn handle_connection(
    stream: TcpStream,
    files: &HashMap<String, Vec<u8>>,
    etags: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    while let Some(request) = read_request(&mut reader).await? {
        let (header, body) = prepare_response(&request, files, etags);
        write_half.write_all(header.as_bytes()).await?;
        if let Some(body) = body {
            write_half.write_all(&body).await?;
        }
        if request.close_after_response {
            break;
        }
    }
    Ok(())
}

/// A parsed HTTP request with just enough understood to serve static files.
struct Request {
    method: String,
    path: String,
    /// `Connection: close`, or no keep-alive candidate (an HTTP/1.0 client
    /// that didn't ask for it).
    close_after_response: bool,
    range: Option<String>,
    accepts_gzip: bool,
    if_none_match: Option<String>,
}

/// Reads one request (request line + headers) from the connection. Returns
/// `Ok(None)` on EOF or on an idle timeout, meaning the connection is done.
async fn read_request(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> std::io::Result<Option<Request>> {
    loop {
        let mut request_line = String::new();
        let Some(n) = read_with_timeout(reader, &mut request_line).await? else {
            return Ok(None);
        };
        if n == 0 {
            return Ok(None);
        }
        // Stray blank lines (keep-alive probes, empty pings) are skipped.
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
        let mut range = None;
        let mut accepts_gzip = false;
        let mut if_none_match = None;

        loop {
            let mut line = String::new();
            let Some(n) = read_with_timeout(reader, &mut line).await? else {
                return Ok(None);
            };
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("connection:") {
                // Explicit header overrides the per-version default.
                close_after_response = value.trim() == "close";
            } else if lower.starts_with("range:") {
                range = Some(line.trim().to_string());
            } else if let Some(value) = lower.strip_prefix("accept-encoding:") {
                accepts_gzip = value.contains("gzip");
            } else if let Some(value) = lower.strip_prefix("if-none-match:") {
                if_none_match = Some(value.trim().to_string());
            }
        }

        return Ok(Some(Request {
            method,
            path,
            close_after_response,
            range,
            accepts_gzip,
            if_none_match,
        }));
    }
}

/// Reads a line, treating an idle timeout as a clean end of connection
/// (`Ok(None)`); a timeout isn't an error worth printing as one.
async fn read_with_timeout(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    buf: &mut String,
) -> std::io::Result<Option<usize>> {
    match tokio::time::timeout(KEEP_ALIVE_IDLE_TIMEOUT, reader.read_line(buf)).await {
        Ok(n) => Ok(Some(n?)),
        Err(_) => Ok(None),
    }
}

/// Resolves the request to what gets served, then builds the full response
/// (header bytes + optional body). GET/HEAD get the HTTP semantics; every
/// other method is answered the same as GET because the server is a read-only
/// static host and method parsing beyond HEAD is out of scope.
fn prepare_response(
    request: &Request,
    files: &HashMap<String, Vec<u8>>,
    etags: &HashMap<String, String>,
) -> (String, Option<Vec<u8>>) {
    // Exact match wins. Otherwise, SPA fallback: a miss for a route-shaped
    // path (no file extension in its last segment) serves the entrypoint so
    // the client-side router can render `/about` etc. on refresh/direct
    // load. A miss for something that *looks like an asset* (e.g.
    // `/assets/broken.js`) stays a real 404, so a broken asset reference
    // isn't silently swaddled in HTML.
    let (served_key, found) = if let Some(body) = files.get(&request.path) {
        (request.path.as_str(), Some(body))
    } else if !is_asset_request(&request.path) {
        match files.get("/") {
            Some(entrypoint) => ("/", Some(entrypoint)),
            None => (request.path.as_str(), None),
        }
    } else {
        (request.path.as_str(), None)
    };

    // Content type is derived from the key actually served, not the
    // requested route — so a fallback to the entrypoint gets `text/html`
    // even though the browser asked for `/about`.
    let content_type = guess_content_type(served_key);
    let connection = if request.close_after_response { "close" } else { "keep-alive" };

    // Conditional GET/HEAD: if the client already holds the representation,
    // answer 304 with no body. Evaluated before Range, per RFC 7232 §6 — a
    // failing precondition makes the request a "precondition fail" regardless
    // of a Range header.
    let is_get_or_head = request.method == "GET" || request.method == "HEAD";
    if let Some(etag) = etags.get(served_key) {
        let matches = request
            .if_none_match
            .as_deref()
            .map(|value| value.trim_matches('"') == etag)
            .unwrap_or(false);
        if is_get_or_head && matches {
            let mut header = String::new();
            push_header(
                &mut header,
                "304 Not Modified",
                content_type,
                None,
                connection,
            );
            header.push_str(&format!("ETag: \"{etag}\"\r\n"));
            header.push_str("\r\n");
            return (header, None);
        }
    }

    let send_body = request.method == "GET";

    let Some(body) = found else {
        return (error_response("404 Not Found", content_type, connection, 0), None);
    };
    let body_len = body.len();

    // Single-range requests (GET only): 206 for a satisfiable range, 416 for
    // an unsatisfiable one. Multi-range and unknown-unit headers are ignored
    // (full 200), which RFC 7233 §3.1 explicitly allows.
    if request.method == "GET" {
        if let Some(range) = parse_range(request.range.as_deref(), body_len) {
            match range {
                RangeSpec::Unsatisfiable => {
                    let mut header = String::new();
                    push_header(
                        &mut header,
                        "416 Range Not Satisfiable",
                        content_type,
                        Some(0),
                        connection,
                    );
                    header.push_str("Accept-Ranges: bytes\r\n");
                    header.push_str(&format!("Content-Range: bytes */{body_len}\r\n"));
                    header.push_str("\r\n");
                    return (header, None);
                }
                RangeSpec::Bytes(start, end) => {
                    let part = body[start..=end].to_vec();
                    let mut header = String::new();
                    push_header(
                        &mut header,
                        "206 Partial Content",
                        content_type,
                        Some(part.len()),
                        connection,
                    );
                    header.push_str("Accept-Ranges: bytes\r\n");
                    header.push_str(&format!("Content-Range: bytes {start}-{end}/{body_len}\r\n"));
                    if let Some(etag) = etags.get(served_key) {
                        header.push_str(&format!("ETag: \"{etag}\"\r\n"));
                    }
                    header.push_str("\r\n");
                    return (header, Some(part));
                }
            }
        }
    }

    // Full representation: gzip compressible types when the client accepts gzip
    // and gzip actually shrinks the body. Range and gzip are never combined —
    // the Range path above returns first, so ranges describe the *stored*
    // (uncompressed) bytes, which is what a parser seeing `Content-Encoding`
    // would expect anyway.
    let mut encoded = None;
    if request.accepts_gzip && body_len >= MIN_COMPRESSIBLE_LEN && is_compressible(content_type) {
        encoded = gzip_body(body).ok().filter(|gz| gz.len() < body_len);
    }
    let content_encoding = if encoded.is_some() { "gzip" } else { "" };
    let payload: &[u8] = encoded.as_deref().unwrap_or(body);

    let mut header = String::new();
    push_header(
        &mut header,
        "200 OK",
        content_type,
        Some(payload.len()),
        connection,
    );
    if let Some(etag) = etags.get(served_key) {
        header.push_str(&format!("ETag: \"{etag}\"\r\n"));
    }
    header.push_str("Accept-Ranges: bytes\r\n");
    // `Vary` is sent for every compressible media type — a shared cache must
    // know a gzipped variant could be served for a different `Accept-Encoding`
    // even when *this* response wasn't compressed. `Content-Encoding` only
    // appears when gzip was actually applied.
    if is_compressible(content_type) {
        header.push_str("Vary: Accept-Encoding\r\n");
        if !content_encoding.is_empty() {
            header.push_str("Content-Encoding: gzip\r\n");
        }
    }
    header.push_str("\r\n");

    let body = if send_body { Some(payload.to_vec()) } else { None };
    (header, body)
}

/// Appends the status line plus the always-present headers to `header`.
/// `content_length` is omitted for 304 (which must not carry a body).
fn push_header(
    header: &mut String,
    status: &str,
    content_type: &str,
    content_length: Option<usize>,
    connection: &str,
) {
    header.push_str("HTTP/1.1 ");
    header.push_str(status);
    header.push_str("\r\nContent-Type: ");
    header.push_str(content_type);
    if let Some(len) = content_length {
        header.push_str(&format!("\r\nContent-Length: {len}"));
    }
    header.push_str(&format!("\r\nConnection: {connection}\r\n"));
}

fn error_response(status: &str, content_type: &str, connection: &str, len: usize) -> String {
    let mut header = String::new();
    push_header(&mut header, status, content_type, Some(len), connection);
    header.push_str("\r\n");
    header
}

enum RangeSpec {
    /// A syntactically valid range no bytes satisfy (e.g. start past EOF).
    Unsatisfiable,
    Bytes(usize, usize),
}

/// Parses a `Range: bytes=A-B` header value into a single inclusive range or
/// `Unsatisfiable`. Returns `None` for headers the server is allowed to
/// ignore (multi-range lists, unknown units, malformed values) — the full 200
/// response is served instead.
fn parse_range(header: Option<&str>, len: usize) -> Option<RangeSpec> {
    // `header` is the raw `Range: bytes=...` line; split the key off so the
    // unit check below sees the value only, in whatever case the client used.
    let (_, spec) = header?.trim().split_once(':')?;
    let spec = spec.trim();
    const UNIT: &str = "bytes=";
    if spec.len() < UNIT.len() || !spec[..UNIT.len()].eq_ignore_ascii_case(UNIT) {
        return None;
    }
    let value = &spec[UNIT.len()..];

    // Multi-range lists are out of scope: ignore (serve 200).
    if value.contains(',') {
        return None;
    }

    let (start_str, end_str) = match value.split_once('-') {
        Some((start, end)) => (start, end),
        None => (value, ""),
    };

    // `bytes=-N`: the last N bytes (suffix range).
    if start_str.is_empty() {
        if len == 0 {
            return Some(RangeSpec::Unsatisfiable);
        }
        let n = end_str.parse::<u64>().ok()?;
        if n == 0 {
            return Some(RangeSpec::Unsatisfiable);
        }
        let take = n.min(len as u64) as usize;
        return Some(RangeSpec::Bytes(len - take, len - 1));
    }

    let start = start_str.parse::<u64>().ok()? as usize;
    if start >= len {
        return Some(RangeSpec::Unsatisfiable);
    }

    let end = if end_str.is_empty() {
        len - 1
    } else {
        // `bytes=A-B`: a malformed start > end is ignored (serve 200).
        let end = end_str.parse::<u64>().ok()? as usize;
        if end < start {
            return None;
        }
        end.min(len - 1)
    };

    Some(RangeSpec::Bytes(start, end))
}

/// gzip-compresses `body` into a new vec (returns the gzip bytes).
fn gzip_body(body: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::with_capacity(body.len()), Compression::default());
    encoder.write_all(body)?;
    encoder.finish()
}

/// Text-ish media types worth compressing; binary formats (fonts, images,
/// video, wasm) are already compressed and get nothing but wasted CPU.
fn is_compressible(content_type: &str) -> bool {
    content_type == "application/javascript"
        || content_type == "application/json"
        || content_type == "image/svg+xml"
        || content_type.starts_with("text/")
}

fn guess_content_type(path: &str) -> &'static str {
    // The entrypoint is served with a path of "/", which has no extension —
    // resolve it to `index.html` so it gets a real HTML content type.
    let name = if path == "/" { "index.html" } else { path };
    match name.rsplit('.').next() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") | Some("mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// True when a request path's last segment carries a file extension, i.e. it
/// is asking for a specific *asset* rather than a client-side route. Used to
/// decide whether a cache miss should fall back to the SPA entrypoint.
fn is_asset_request(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    last.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use canopee_storage::ObjectId;
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Starts a server on an ephemeral port with `files` and returns its addr.
    async fn spawn_server(files: HashMap<String, Vec<u8>>) -> std::net::SocketAddr {
        let etags = compute_etags(&files);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let addr = listener.local_addr().unwrap();
            tx.send(addr).unwrap();
            let files = Arc::new(files);
            let etags = Arc::new(etags);
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let files = files.clone();
                let etags = etags.clone();
                tokio::spawn(async move {
                    handle_connection(stream, &files, &etags).await.unwrap();
                });
            }
        });
        rx.await.unwrap()
    }

    fn sample_files() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert("/".to_string(), b"<div id=root>app</div>".to_vec());
        files.insert("/style.css".to_string(), format!("h1{{color:red}}/*{}*/", "a".repeat(500)).into_bytes());
        files.insert("/assets/app.js".to_string(), b"console.log('hi')".to_vec());
        files.insert("/video.mp4".to_string(), (0..10u8).collect());
        files.insert("/img.png".to_string(), std::iter::repeat_n(b'\xff', 512).collect());
        files
    }

    #[tokio::test]
    async fn serves_index_and_assets() {
        let addr = spawn_server(sample_files()).await;

        let (status, body) = http_request(&addr, "/").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(body.contains("app"));
        let (status, css) = http_request(&addr, "/style.css").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(css.contains("color:red"));
    }

    #[tokio::test]
    async fn spa_fallback_serves_entrypoint_for_routes_and_404s_missing_assets() {
        let addr = spawn_server(sample_files()).await;

        // Route-shaped path missing from `files` -> served the entrypoint.
        let (status, content_type, body) = http_request_with_ct(&addr, "/about").await;
        assert!(status.contains(" 200 "), "route should fall back to index: {status}");
        assert!(body.contains("app"));
        assert!(content_type.starts_with("text/html"), "fallback body must be marked html, got {content_type}");

        // Nested route with a query string.
        let (status, body) = http_request(&addr, "/users/42?tab=posts").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(body.contains("app"));

        // Missing *asset-looking* path stays a real 404.
        let (status, _content_type, _body) =
            http_request_with_ct(&addr, "/assets/does-not-exist.js").await;
        assert!(status.contains(" 404 "), "missing asset must not silently fall back");
    }

    #[tokio::test]
    async fn range_requests_serve_partial_content() {
        let addr = spawn_server(sample_files()).await;

        // `bytes=A-B`: inclusive both ends.
        let (status, headers, body) =
            http_request_full(&addr, "GET", "/video.mp4", &[("Range", "bytes=2-4"), ("Connection", "close")]).await;
        assert!(status.contains(" 206 "), "got {status}");
        assert_eq!(headers["content-range"], "bytes 2-4/10");
        assert_eq!(body, vec![2, 3, 4]);

        // `bytes=A-`: to the end of the file.
        let (_status, _headers, body) =
            http_request_full(&addr, "GET", "/video.mp4", &[("Range", "bytes=7-"), ("Connection", "close")]).await;
        assert_eq!(body, vec![7, 8, 9]);

        // `bytes=-N`: the last N bytes.
        let (_status, headers, body) =
            http_request_full(&addr, "GET", "/video.mp4", &[("Range", "bytes=-4"), ("Connection", "close")]).await;
        assert_eq!(headers["content-range"], "bytes 6-9/10");
        assert_eq!(body, vec![6, 7, 8, 9]);

        // Unsatisfiable range -> 416 with a byte *unknown* size.
        let (status, headers, _body) =
            http_request_full(&addr, "GET", "/video.mp4", &[("Range", "bytes=20-"), ("Connection", "close")]).await;
        assert!(status.contains(" 416 "), "got {status}");
        assert_eq!(headers["content-range"], "bytes */10");

        // Every 200/206 advertises range support.
        let (_status, headers, _body) =
            http_request_full(&addr, "GET", "/video.mp4", &[("Connection", "close")]).await;
        assert_eq!(headers["accept-ranges"], "bytes");
    }

    #[tokio::test]
    async fn gzip_compresses_compressible_assets_when_accepted() {
        let addr = spawn_server(sample_files()).await;

        let (status, headers, body) = http_request_full(
            &addr,
            "GET",
            "/style.css",
            &[("Accept-Encoding", "gzip"), ("Connection", "close")],
        )
        .await;
        assert!(status.contains(" 200 "), "got {status}");
        assert_eq!(headers["content-encoding"], "gzip");
        assert_eq!(headers["vary"], "Accept-Encoding");

        let mut decoded = Vec::new();
        GzDecoder::new(&body[..]).read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, sample_files()["/style.css"]);

        // Not compressed when the client doesn't ask, but still `Vary`s so a shared
        // cache knows a gzip variant could appear; already-binary types get
        // neither.
        let (_status, headers, body) =
            http_request_full(&addr, "GET", "/style.css", &[("Connection", "close")]).await;
        assert!(!headers.contains_key("content-encoding"));
        assert_eq!(headers["vary"], "Accept-Encoding");
        assert_eq!(body, sample_files()["/style.css"]);

        let (_status, headers, _body) = http_request_full(
            &addr,
            "GET",
            "/img.png",
            &[("Accept-Encoding", "gzip"), ("Connection", "close")],
        )
        .await;
        assert!(!headers.contains_key("content-encoding"), "png must not be gzipped");
        assert!(!headers.contains_key("vary"), "png must not advertise gzip variants");
    }

    #[tokio::test]
    async fn head_returns_headers_without_body() {
        let addr = spawn_server(sample_files()).await;

        let (status, headers, body) =
            http_request_full(&addr, "HEAD", "/style.css", &[("Connection", "close")]).await;
        assert!(status.contains(" 200 "), "got {status}");
        let get_len = headers["content-length"].parse::<usize>().unwrap();
        assert!(body.is_empty(), "HEAD must not send a body");

        // The reported length must match what a GET would deliver.
        let (_status, get_headers, get_body) =
            http_request_full(&addr, "GET", "/style.css", &[("Connection", "close")]).await;
        assert_eq!(get_len, get_body.len());
        assert_eq!(
            get_headers["content-length"].parse::<usize>().unwrap(),
            get_body.len(),
            "GET must deliver exactly the Content-Length HEAD advertised"
        );
    }

    #[tokio::test]
    async fn etag_and_conditional_get_return_304() {
        let addr = spawn_server(sample_files()).await;

        let (_status, headers, body) =
            http_request_full(&addr, "GET", "/", &[("Connection", "close")]).await;
        let etag = headers["etag"].clone();
        assert!(etag.starts_with('"'));
        assert!(!body.is_empty());

        // A GET that declares it already has the etag -> 304, no body.
        let (status, headers, body) = http_request_full(
            &addr,
            "GET",
            "/",
            &[("If-None-Match", &etag), ("Connection", "close")],
        )
        .await;
        assert!(status.contains(" 304 "), "got {status}");
        assert!(body.is_empty());
        assert_eq!(headers["etag"], etag);

        // A *different* etag -> full 200.
        let (status, body) = http_request(&addr, "/").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(body.contains("app"));

        // The etag is the object's content identity (SHA-256), so a file whose
        // bytes are already content-addressed matches compute_etags directly.
        let derived = format!("\"{}\"", ObjectId::from_data(&sample_files()["/"]).0);
        assert_eq!(etag, derived);
    }

    #[tokio::test]
    async fn keep_alive_serves_multiple_requests_on_one_connection() {
        let addr = spawn_server(sample_files()).await;

        let mut stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(read_half);

        write_half
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n")
            .await
            .unwrap();
        let (status, _headers, body) = read_response(&mut reader).await;
        assert!(status.contains(" 200 "));
        assert!(String::from_utf8_lossy(&body).contains("app"));

        // Second request on the same connection.
        write_half
            .write_all(b"GET /style.css HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let (status, headers, body) = read_response(&mut reader).await;
        assert!(status.contains(" 200 "), "got {status}");
        assert_eq!(headers["connection"], "close");
        assert_eq!(body, sample_files()["/style.css"]);
    }

    #[test]
    fn content_types_cover_common_bundler_output() {
        for (path, expected) in [
            ("/", "text/html; charset=utf-8"),
            ("/index.html", "text/html; charset=utf-8"),
            ("/style.css", "text/css"),
            ("/app.js", "application/javascript"),
            ("/app.mjs", "application/javascript"),
            ("/app.json", "application/json"),
            ("/app.js.map", "application/json"),
            ("/LICENSE.txt", "text/plain; charset=utf-8"),
            ("/img.png", "image/png"),
            ("/img.webp", "image/webp"),
            ("/img.avif", "image/avif"),
            ("/img.gif", "image/gif"),
            ("/img.svg", "image/svg+xml"),
            ("/favicon.ico", "image/x-icon"),
            ("/font.woff", "font/woff"),
            ("/font.woff2", "font/woff2"),
            ("/font.ttf", "font/ttf"),
            ("/font.otf", "font/otf"),
            ("/font.eot", "application/vnd.ms-fontobject"),
            ("/unknown.zzz", "application/octet-stream"),
        ] {
            assert_eq!(guess_content_type(path), expected, "mismatch for {path}");
        }
    }

    #[test]
    fn is_asset_request_distinguishes_routes_from_assets() {
        assert!(!is_asset_request("/"));
        assert!(!is_asset_request("/about"));
        assert!(!is_asset_request("/users/42"));
        assert!(!is_asset_request("/users/42?tab=posts"));
        assert!(!is_asset_request("/about/"));
        assert!(is_asset_request("/assets/app.js"));
        assert!(is_asset_request("/assets/app.js.map"));
        assert!(is_asset_request("/favicon.ico"));
    }

    #[test]
    fn publish_catches_non_root_build_base() {
        // HTML referencing assets under a base path that was never published.
        let html = r#"
            <html>
              <link rel="icon" href="/my-app/favicon.ico">
              <script src="/my-app/assets/index-a1b2c3.js"></script>
              <script src="//external.example/vendor.js"></script>
            </html>
        "#;
        let published = vec![
            ("/assets/index-a1b2c3.js".to_string(), ObjectId::new("/assets/index-a1b2c3.js")),
            ("/favicon.ico".to_string(), ObjectId::new("/favicon.ico")),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        // A correct build (root-relative refs all published) passes.
        let ok_html = r#"<script src="/assets/index-a1b2c3.js"></script>"#;
        assert!(unpublished_asset_references(ok_html, &published).is_empty());

        let missing = unpublished_asset_references(html, &published);
        assert_eq!(
            missing,
            vec!["/my-app/assets/index-a1b2c3.js", "/my-app/favicon.ico"]
        );
    }

    /// Sends one request over a fresh connection (default `Connection: close`)
    /// and reads the whole response into (status line, header map, body).
    /// `method` is `"GET"` (or `"HEAD"` for header-only requests).
    async fn http_request_full(
        addr: &std::net::SocketAddr,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (String, HashMap<String, String>, Vec<u8>) {
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n");
        for (k, v) in headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");

        let mut stream = TcpStream::connect(*addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        parse_response(&response)
    }

    /// Parses a captured response into (status line, headers, body bytes).
    fn parse_response(response: &[u8]) -> (String, HashMap<String, String>, Vec<u8>) {
        let head_end = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(response.len());
        let head = std::str::from_utf8(&response[..head_end]).unwrap_or("");
        let body = response[head_end..].to_vec();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or("").to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((key, value)) = line.split_once(':') {
                headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        (status, headers, body)
    }

    /// Reads one complete HTTP response from an existing connection, honoring
    /// `Content-Length` so a keep-alive connection stays usable afterwards.
    async fn read_response(
        reader: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    ) -> (String, HashMap<String, String>, Vec<u8>) {
        let mut head = String::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await.unwrap();
            if n == 0 {
                break;
            }
            head.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let (status, mut headers, _) = parse_response(head.as_bytes());
        let mut body = Vec::new();
        if status.contains(" 200 ") || status.contains(" 206 ") || status.contains(" 416 ") {
            let len = headers
                .get("content-length")
                .map(|l| l.parse::<usize>().unwrap())
                .unwrap_or(0);
            body.resize(len, 0);
            reader.read_exact(&mut body).await.unwrap();
        }
        (status, headers, body)
    }

    async fn http_request(addr: &std::net::SocketAddr, path: &str) -> (String, String) {
        let (status, _, body) = http_request_full(addr, "GET", path, &[("Connection", "close")]).await;
        (status, String::from_utf8_lossy(&body).to_string())
    }

    async fn http_request_with_ct(
        addr: &std::net::SocketAddr,
        path: &str,
    ) -> (String, String, String) {
        let (status, headers, body) =
            http_request_full(addr, "GET", path, &[("Connection", "close")]).await;
        let content_type = headers.get("content-type").cloned().unwrap_or_default();
        (status, content_type, String::from_utf8_lossy(&body).to_string())
    }
}
