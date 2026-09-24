//! Static-file HTTP semantics shared by every served Canopee app, factored out
//! of the CLI's local `serve` command so the P2P serve session can reuse it
//! verbatim over the edge tunnel. Pure functions: given a request
//! (method/path/headers) and the app's file map, produce a `ServeResponse`
//! the edge relays to the browser unchanged.

use canopee_network::ServeResponse;
use canopee_storage::ObjectId;
use std::collections::HashMap;

/// Bodies smaller than this are not worth gzipping — the gzip header and
/// trailer alone can exceed the savings.
const MIN_COMPRESSIBLE_LEN: usize = 256;

/// Builds the complete HTTP response for one request. `method` is the
/// verb, `path` the app-relative request target (the serve session has
/// already stripped the app name), and the header lookup uses only the
/// forwarded subset (see [`SERVE_FORWARDED_HEADERS`]).
pub fn render_response(
    files: &HashMap<String, Vec<u8>>,
    method: &str,
    path: &str,
    headers: &[(String, String)],
) -> ServeResponse {
    let mut response = ServeResponse::default();

    let accepts_gzip = header(headers, "accept-encoding")
        .map(|v| v.contains("gzip"))
        .unwrap_or(false);
    let range = header(headers, "range").map(str::to_string);
    let if_none_match = header(headers, "if-none-match").map(str::to_string);

    // Exact match wins. Otherwise, SPA fallback: a miss for a route-shaped
    // path (no file extension in its last segment) serves the entrypoint so
    // the client-side router can render `/about` etc. on refresh/direct
    // load. A miss for something that *looks like an asset* (e.g.
    // `/assets/broken.js`) stays a real 404, so a broken asset reference
    // isn't silently swaddled in HTML.
    let (served_key, found) = match files.get(path) {
        Some(body) => (path.to_string(), Some(body)),
        None if !is_asset_request(path) => match files.get("/") {
            Some(entrypoint) => ("/".to_string(), Some(entrypoint)),
            None => (path.to_string(), None),
        },
        None => (path.to_string(), None),
    };

    // Content type is derived from the key actually served, not the
    // requested route — so a fallback to the entrypoint gets `text/html`
    // even though the browser asked for `/about`.
    let content_type = guess_content_type(&served_key);

    // Conditional GET/HEAD: if the client already holds the representation,
    // answer 304 with no body. Evaluated before Range, per RFC 7232 §6 — a
    // failing precondition makes the request a "precondition fail" regardless
    // of a Range header. Only meaningful when a file is found: a missing
    // object must never answer 304.
    let is_get_or_head = method == "GET" || method == "HEAD";
    let etag_value = found.map(|b| format!("\"{}\"", ObjectId::from_data(b).0));
    let matches = match (&etag_value, &if_none_match) {
        (Some(ours), Some(sent)) => sent.trim_matches('"').trim() == ours.trim_matches('"'),
        _ => false,
    };
    if is_get_or_head && matches {
        push_header(&mut response.headers, "Content-Type", content_type);
        if let Some(etag_value) = &etag_value {
            push_header(&mut response.headers, "ETag", etag_value);
        }
        response.status = "304 Not Modified".into();
        return response;
    }

    let send_body = method == "GET";

    let Some(body) = found else {
        push_header(&mut response.headers, "Content-Type", content_type);
        response.status = "404 Not Found".into();
        return response;
    };
    let body_len = body.len();

    // Single-range requests (GET only): 206 for a satisfiable range, 416 for
    // an unsatisfiable one. Multi-range and unknown-unit headers are ignored
    // (full 200), which RFC 7233 §3.1 explicitly allows.
    if method == "GET" {
        if let Some(range) = parse_range(range.as_deref(), body_len) {
            match range {
                RangeSpec::Unsatisfiable => {
                    response.status = "416 Range Not Satisfiable".into();
                    push_header(&mut response.headers, "Content-Type", content_type);
                    push_header(&mut response.headers, "Accept-Ranges", "bytes");
                    push_header(
                        &mut response.headers,
                        "Content-Range",
                        &format!("bytes */{body_len}"),
                    );
                    return response;
                }
                RangeSpec::Bytes(start, end) => {
                    response.status = "206 Partial Content".into();
                    push_header(&mut response.headers, "Content-Type", content_type);
                    push_header(
                        &mut response.headers,
                        "Content-Length",
                        &part_len_text(end - start + 1),
                    );
                    push_header(&mut response.headers, "Accept-Ranges", "bytes");
                    push_header(
                        &mut response.headers,
                        "Content-Range",
                        &format!("bytes {start}-{end}/{body_len}"),
                    );
                    if let Some(etag_value) = &etag_value {
                        push_header(&mut response.headers, "ETag", etag_value);
                    }
                    response.body = body[start..=end].to_vec();
                    return response;
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
    if accepts_gzip && body_len >= MIN_COMPRESSIBLE_LEN && is_compressible(content_type) {
        encoded = gzip_body(body).ok().filter(|gz| gz.len() < body_len);
    }
    let payload: &[u8] = encoded.as_deref().unwrap_or(body);

    response.status = "200 OK".into();
    push_header(&mut response.headers, "Content-Type", content_type);
    push_header(
        &mut response.headers,
        "Content-Length",
        &payload.len().to_string(),
    );
    if let Some(etag_value) = &etag_value {
        push_header(&mut response.headers, "ETag", etag_value);
    }
    push_header(&mut response.headers, "Accept-Ranges", "bytes");
    // `Vary` is sent for every compressible media type — a shared cache must
    // know a gzipped variant could be served for a different `Accept-Encoding`
    // even when *this* response wasn't compressed. `Content-Encoding` only
    // appears when gzip was actually applied.
    if is_compressible(content_type) {
        push_header(&mut response.headers, "Vary", "Accept-Encoding");
        if encoded.is_some() {
            push_header(&mut response.headers, "Content-Encoding", "gzip");
        }
    }
    if send_body {
        response.body = payload.to_vec();
    }
    response
}

fn part_len_text(len: usize) -> String {
    len.to_string()
}

fn push_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.push((name.to_string(), value.to_string()));
}

fn header<'a>(headers: &'a [(String, String)], want: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(want))
        .map(|(_, v)| v.as_str())
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
    // Accept either the bare value (`bytes=2-4`) or a full header line
    // (`Range: bytes=2-4`); strip a leading key if present so the unit check
    // below sees the value only, in whatever case the client used.
    let spec = header?.trim();
    let spec = match spec.split_once(':') {
        Some((_, value)) => value.trim(),
        None => spec,
    };
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

    fn sample_files() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert("/".to_string(), b"<div id=root>app</div>".to_vec());
        files.insert(
            "/style.css".to_string(),
            format!("h1{{color:red}}/*{}*/", "a".repeat(500)).into_bytes(),
        );
        files.insert("/assets/app.js".to_string(), b"console.log('hi')".to_vec());
        files.insert("/video.mp4".to_string(), (0..10u8).collect());
        files.insert(
            "/img.png".to_string(),
            std::iter::repeat_n(b'\xff', 512).collect(),
        );
        files
    }

    fn get(files: &HashMap<String, Vec<u8>>, method: &str, path: &str) -> ServeResponse {
        render_response(files, method, path, &[])
    }

    fn resp_header<'a>(r: &'a ServeResponse, name: &str) -> &'a str {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    #[test]
    fn serves_index_and_assets() {
        let files = sample_files();
        let index = get(&files, "GET", "/");
        assert_eq!(index.status, "200 OK");
        assert!(String::from_utf8_lossy(&index.body).contains("app"));

        let css = get(&files, "GET", "/style.css");
        assert_eq!(css.status, "200 OK");
        assert!(String::from_utf8_lossy(&css.body).contains("color:red"));
    }

    #[test]
    fn spa_fallback_serves_entrypoint_for_routes_and_404s_missing_assets() {
        let files = sample_files();

        let route = get(&files, "GET", "/about");
        assert_eq!(route.status, "200 OK");
        assert!(String::from_utf8_lossy(&route.body).contains("app"));
        assert_eq!(
            resp_header(&route, "content-type"),
            "text/html; charset=utf-8"
        );

        let qs = get(&files, "GET", "/users/42?tab=posts");
        assert_eq!(qs.status, "200 OK");

        let missing_asset = get(&files, "GET", "/assets/does-not-exist.js");
        assert_eq!(missing_asset.status, "404 Not Found");
    }

    #[test]
    fn missing_file_never_returns_304_even_with_matching_etag_header() {
        let files = sample_files();
        let headers = vec![(
            "If-None-Match".to_string(),
            format!("\"{}\"", ObjectId::from_data(b"").0),
        )];
        let r = render_response(&files, "GET", "/assets/not-there.js", &headers);
        assert_eq!(r.status, "404 Not Found");
    }

    #[test]
    fn range_requests_serve_partial_content() {
        let files = sample_files();
        let headers = vec![("Range".to_string(), "bytes=2-4".to_string())];
        let r = render_response(&files, "GET", "/video.mp4", &headers);
        assert_eq!(r.status, "206 Partial Content");
        assert_eq!(resp_header(&r, "content-range"), "bytes 2-4/10");
        assert_eq!(r.body, vec![2, 3, 4]);

        let headers = vec![("Range".to_string(), "bytes=-4".to_string())];
        let r = render_response(&files, "GET", "/video.mp4", &headers);
        assert_eq!(resp_header(&r, "content-range"), "bytes 6-9/10");
        assert_eq!(r.body, vec![6, 7, 8, 9]);

        let headers = vec![("Range".to_string(), "bytes=20-".to_string())];
        let r = render_response(&files, "GET", "/video.mp4", &headers);
        assert_eq!(r.status, "416 Range Not Satisfiable");
        assert_eq!(resp_header(&r, "content-range"), "bytes */10");
    }

    #[test]
    fn gzip_compresses_compressible_assets_when_accepted() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let files = sample_files();
        let headers = vec![("accept-encoding".to_string(), "gzip".to_string())];
        let r = render_response(&files, "GET", "/style.css", &headers);
        assert_eq!(r.status, "200 OK");
        assert_eq!(resp_header(&r, "content-encoding"), "gzip");
        assert_eq!(resp_header(&r, "vary"), "Accept-Encoding");

        let mut decoded = Vec::new();
        GzDecoder::new(&r.body[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, sample_files()["/style.css"]);

        // No gzip unless the client asks; binary types never compress.
        let plain = get(&files, "GET", "/style.css");
        assert_eq!(resp_header(&plain, "vary"), "Accept-Encoding");
        assert!(!plain
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding")));

        let png = render_response(&files, "GET", "/img.png", &headers);
        assert!(!png
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding")));
        assert!(!png
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("vary")));
    }

    #[test]
    fn head_returns_headers_without_body() {
        let files = sample_files();
        let r = get(&files, "HEAD", "/style.css");
        assert_eq!(r.status, "200 OK");
        assert!(r.body.is_empty());
        let get = get(&files, "GET", "/style.css");
        let get_len = resp_header(&get, "content-length");
        assert_eq!(resp_header(&r, "content-length"), get_len);
    }

    #[test]
    fn etag_and_conditional_get_return_304() {
        let files = sample_files();
        let root = get(&files, "GET", "/");
        let etag = resp_header(&root, "etag");
        assert!(etag.starts_with('"'));

        let headers = vec![("If-None-Match".to_string(), etag.to_string())];
        let r = render_response(&files, "GET", "/", &headers);
        assert_eq!(r.status, "304 Not Modified");
        assert!(r.body.is_empty());

        let derived = format!("\"{}\"", ObjectId::from_data(&sample_files()["/"]).0);
        assert_eq!(etag, derived);
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
            ("/img.png", "image/png"),
            ("/font.woff2", "font/woff2"),
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
        assert!(!is_asset_request("/about/"));
        assert!(is_asset_request("/assets/app.js"));
        assert!(is_asset_request("/favicon.ico"));
    }
}
