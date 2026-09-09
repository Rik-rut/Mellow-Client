//! Per-server local HTTP reverse proxy.
//!
//! On macOS/Linux (and on Windows when MELLOW_PROXY=1) the webview navigates to
//! `http://127.0.0.1:<port>` instead of the server's real `https://` URL, so
//! WebKit gets a secure context (localhost) and getUserMedia/WebSocket work
//! without the OS refusing the server's self-signed certificate.
//!
//! Forwards everything — streaming bodies and WebSocket upgrades included —
//! to the upstream over TLS with certificate verification disabled (same LAN
//! trust model WebView2's --ignore-certificate-errors already uses).

use std::io::Error as IoError;
use std::net::IpAddr;
use std::sync::Arc;

use futures_util::stream::StreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::header::{HeaderName, HeaderValue, HOST, LOCATION, SET_COOKIE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

const PROXY_PORT_BASE: u32 = 21500;
const PROXY_PORT_SPAN: u32 = 10000;

/// Hop-by-hop headers that must not be forwarded in either direction.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
];

type ProxyRes = Response<BoxBody<Bytes, IoError>>;

/// Bind a proxy for `target_url` (e.g. `https://192.168.1.5:6767`).
/// Returns the local port; the accept loop runs as a spawned task for the
/// lifetime of the process (one proxy per distinct server URL).
pub async fn start_proxy(target_url: &str) -> Result<u16, String> {
    let upstream = Target::parse(target_url)?;
    let connector = tls_connector()?;

    let preferred = PROXY_PORT_BASE + (stable_hash(target_url) % PROXY_PORT_SPAN);
    let listener = match TcpListener::bind(("127.0.0.1", preferred as u16)).await {
        Ok(l) => l,
        // stable port busy (another process / stale): let the OS assign one.
        // Worst case is a one-time re-login because the origin changed.
        Err(_) => TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| format!("proxy bind: {e}"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|e| format!("proxy addr: {e}"))?
        .port();

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let upstream = upstream.clone();
                    let connector = connector.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let upstream = upstream.clone();
                            let connector = connector.clone();
                            async move { route(req, upstream, connector).await }
                        });
                        let _ = http1::Builder::new()
                            .title_case_headers(true)
                            .serve_connection(io, svc)
                            .with_upgrades()
                            .await;
                    });
                }
                Err(_) => break,
            }
        }
    });

    Ok(port)
}

/// Parsed + owned upstream info.
#[derive(Clone)]
struct Target {
    host: String,
    port: u16,
    authority: String, // host:port for Host header / TCP connect
}

impl Target {
    fn parse(url: &str) -> Result<Self, String> {
        let idx = url.find("://").ok_or("proxy: invalid url")?;
        let scheme = &url[..idx];
        if scheme != "https" && scheme != "http" {
            return Err("proxy: unsupported scheme".into());
        }
        let rest = url[idx + 3..].trim_end_matches('/');
        let authority = match rest.find('/') {
            Some(i) => &rest[..i],
            None => rest,
        };
        let (host, port) = split_authority(authority, if scheme == "https" { 443 } else { 80 });
        Ok(Target {
            host: host.to_string(),
            port,
            authority: authority.to_string(),
        })
    }
}

fn split_authority(authority: &str, default_port: u16) -> (&str, u16) {
    // IPv6 literal: [::1]:6767
    if let Some(stripped) = authority.strip_prefix('[') {
        if let Some(i) = stripped.find(']') {
            // i is relative to stripped (`::1]...`); `]` sits at i+1 in authority.
            let host = &authority[..i + 2];
            let port = stripped[i + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(default_port);
            return (host, port);
        }
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() => (h, p.parse().unwrap()),
        _ => (authority, default_port),
    }
}

fn stable_hash(s: &str) -> u32 {
    let digest = Sha256::digest(s.as_bytes());
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/* ───────────────────────────── TLS (no verify) ────────────────────────── */

#[derive(Debug)]
struct NoCertVerifier {
    supported: WebPkiSupportedAlgorithms,
}

impl NoCertVerifier {
    fn new() -> Self {
        Self {
            supported: ring::default_provider().signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported.supported_schemes()
    }
}

fn tls_connector() -> Result<TlsConnector, String> {
    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier::new()))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

fn server_name(host: &str) -> ServerName<'static> {
    let bare = host.trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return ServerName::IpAddress(ip.into()).to_owned();
    }
    ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap())
        .to_owned()
}

/* ───────────────────────────── request routing ────────────────────────── */

async fn route(req: Request<Incoming>, upstream: Target, connector: TlsConnector) -> Result<ProxyRes, hyper::Error> {
    if is_upgrade(&req) {
        handle_upgrade(req, upstream, connector).await
    } else {
        handle_http(req, upstream, connector).await
    }
}

fn is_upgrade<B>(req: &Request<B>) -> bool {
    let conn_up = req
        .headers()
        .get_all(hyper::header::CONNECTION)
        .iter()
        .any(|v| {
            v.to_str()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default()
                .contains("upgrade")
        });
    conn_up && req.headers().contains_key("upgrade")
}

fn local_origin<B>(req: &Request<B>) -> String {
    let host = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1");
    format!("http://{host}")
}

/* ───────────────────────── plain HTTP forwarding ──────────────────────── */

async fn handle_http(
    req: Request<Incoming>,
    upstream: Target,
    connector: TlsConnector,
) -> Result<ProxyRes, hyper::Error> {
    let origin = local_origin(&req);
    let method = req.method().clone();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // Rebuild for upstream: same headers minus hop-by-hop, Host rewritten to
    // the upstream authority (server sees its own host).
    let mut builder = Request::builder().method(method).uri(path_and_query);
    {
        let headers = builder.headers_mut().unwrap();
        copy_headers(req.headers(), headers);
        headers.insert(
            HOST,
            HeaderValue::from_str(&upstream.authority)
                .unwrap_or_else(|_| HeaderValue::from_static("localhost")),
        );
    }
    let upstream_req = match builder.body(req.into_body()) {
        Ok(r) => r,
        Err(e) => return Ok(error_response(StatusCode::BAD_GATEWAY, &format!("proxy build: {e}"))),
    };

    let stream = match connect_tls(&upstream, connector).await {
        Ok(s) => s,
        Err(e) => return Ok(error_response(StatusCode::BAD_GATEWAY, &e)),
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .handshake(io)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                &format!("proxy handshake: {e}"),
            ))
        }
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let res = match sender.send_request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                &format!("proxy request: {e}"),
            ))
        }
    };

    Ok(rewrite_response(res, &origin))
}

fn copy_headers(from: &hyper::HeaderMap, to: &mut hyper::HeaderMap) {
    for (name, value) in from.iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lname.as_str()) || name == HOST {
            continue;
        }
        to.append(name.clone(), value.clone());
    }
}

fn rewrite_response(res: Response<Incoming>, origin: &str) -> ProxyRes {
    let (parts, body) = res.into_parts();
    let mut headers = hyper::HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lname.as_str()) {
            continue;
        }
        if name == LOCATION {
            headers.append(name.clone(), rewrite_location(value, origin));
            continue;
        }
        if name == SET_COOKIE {
            headers.append(name.clone(), rewrite_set_cookie(value));
            continue;
        }
        headers.append(name.clone(), value.clone());
    }

    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(IoError::other).map(Frame::data));

    let mut res = Response::new(BodyExt::boxed(StreamBody::new(stream)));
    *res.status_mut() = parts.status;
    *res.version_mut() = parts.version;
    *res.headers_mut() = headers;
    res
}

fn rewrite_location(value: &HeaderValue, origin: &str) -> HeaderValue {
    let s = match value.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return value.clone(),
    };
    // Only absolute URLs carry the upstream origin; relative Locations stay as-is.
    for prefix in ["https://", "http://"] {
        if s.starts_with(prefix) {
            if let Some(i) = s[prefix.len()..].find('/') {
                let rest = &s[prefix.len() + i..];
                return HeaderValue::from_str(&format!("{origin}{rest}")).unwrap_or(value.clone());
            }
            return HeaderValue::from_str(origin).unwrap_or(value.clone());
        }
    }
    value.clone()
}

fn rewrite_set_cookie(value: &HeaderValue) -> HeaderValue {
    let s = match value.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return value.clone(),
    };
    let mut out = String::with_capacity(s.len());
    let mut dropped_secure = false;
    for (i, attr) in s.split("; ").enumerate() {
        let lower = attr.to_ascii_lowercase();
        if i == 0 {
            out.push_str(attr);
            continue;
        }
        if lower == "secure" {
            dropped_secure = true;
            continue;
        }
        if lower == "samesite=none" && dropped_secure {
            // SameSite=None is invalid without Secure on the local origin.
            out.push_str("; SameSite=Lax");
            continue;
        }
        if let Some((k, _v)) = lower.split_once('=') {
            if k == "domain" {
                out.push_str("; Domain=127.0.0.1");
                continue;
            }
        }
        out.push_str("; ");
        out.push_str(attr);
    }
    HeaderValue::from_str(&out).unwrap_or(value.clone())
}

/* ───────────────────────── WebSocket (upgrade) path ───────────────────── */

async fn handle_upgrade(
    mut req: Request<Incoming>,
    upstream: Target,
    connector: TlsConnector,
) -> Result<ProxyRes, hyper::Error> {
    // Capture the client-side upgrade future before consuming `req`.
    let client_upgrade = hyper::upgrade::on(&mut req);

    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // Serialize the handshake request for the upstream, keeping the
    // Connection/Upgrade pair and rewriting only Host.
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method().as_str(), path_and_query);
    for (name, value) in req.headers().iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if lname == "host" || (HOP_BY_HOP.contains(&lname.as_str()) && lname != "connection") {
            continue;
        }
        if let Ok(v) = value.to_str() {
            head.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    head.push_str(&format!("Host: {}\r\n\r\n", upstream.authority));

    let stream = match connect_tls(&upstream, connector).await {
        Ok(s) => s,
        Err(e) => return Ok(error_response(StatusCode::BAD_GATEWAY, &e)),
    };
    let mut reader = BufReader::new(stream);
    if let Err(e) = reader.write_all(head.as_bytes()).await {
        return Ok(error_response(
            StatusCode::BAD_GATEWAY,
            &format!("proxy ws write: {e}"),
        ));
    }
    if reader.flush().await.is_err() {
        return Ok(error_response(StatusCode::BAD_GATEWAY, "proxy ws flush"));
    }

    // Read the upstream handshake response head (leftover bytes stay in the
    // BufReader and become the first WebSocket frames).
    let mut status = String::new();
    match reader.read_line(&mut status).await {
        Ok(0) | Err(_) => {
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "proxy ws: closed by upstream",
            ))
        }
        Ok(_) => {}
    }
    if !status.contains(" 101 ") {
        return Ok(error_response(
            StatusCode::BAD_GATEWAY,
            &format!("proxy ws: upstream said {}", status.trim()),
        ));
    }
    let mut resp_headers = Vec::new();
    loop {
        let mut l = String::new();
        match reader.read_line(&mut l).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = l.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            if let (Ok(kn), Ok(vv)) = (
                HeaderName::from_bytes(k.trim().as_bytes()),
                HeaderValue::from_str(v.trim()),
            ) {
                resp_headers.push((kn, vv));
            }
        }
    }

    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    {
        let hs = builder.headers_mut().unwrap();
        for (k, v) in resp_headers {
            hs.append(k, v);
        }
    }
    let res = builder
        .body(Empty::<Bytes>::new().map_err(|e| match e {}).boxed())
        .unwrap();

    // Bridge both directions once hyper completes the local-side upgrade.
    tokio::spawn(async move {
        if let Ok(upgraded) = client_upgrade.await {
            let mut upgraded = TokioIo::new(upgraded);
            let _ = tokio::io::copy_bidirectional(&mut upgraded, &mut reader).await;
        }
    });

    Ok(res)
}

/* ──────────────────────────────── helpers ─────────────────────────────── */

async fn connect_tls(
    upstream: &Target,
    connector: TlsConnector,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
    let tcp = TcpStream::connect(upstream.authority.as_str())
        .await
        .map_err(|e| format!("proxy connect {}:{}: {e}", upstream.host, upstream.port))?;
    let name = server_name(&upstream.host);
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| format!("proxy tls {}:{}: {e}", upstream.host, upstream.port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parse_host_port() {
        let t = Target::parse("https://192.168.1.5:6767").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.authority.as_str()), ("192.168.1.5", 6767, "192.168.1.5:6767"));
        let t = Target::parse("https://chat.lan/").unwrap();
        assert_eq!((t.host.as_str(), t.port), ("chat.lan", 443));
        let t = Target::parse("http://[::1]:6767").unwrap();
        assert_eq!((t.host.as_str(), t.port), ("[::1]", 6767));
        assert!(Target::parse("ftp://x").is_err());
    }

    #[test]
    fn stable_port_is_deterministic_and_in_range() {
        let a = PROXY_PORT_BASE + (stable_hash("https://192.168.1.5:6767") % PROXY_PORT_SPAN);
        let b = PROXY_PORT_BASE + (stable_hash("https://192.168.1.5:6767") % PROXY_PORT_SPAN);
        let c = PROXY_PORT_BASE + (stable_hash("https://192.168.1.9:6767") % PROXY_PORT_SPAN);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!((21500..31500).contains(&a));
    }

    #[test]
    fn location_absolute_rewritten_relative_kept() {
        let v = rewrite_location(
            &HeaderValue::from_static("https://192.168.1.5:6767/login?x=1"),
            "http://127.0.0.1:21500",
        );
        assert_eq!(v.to_str().unwrap(), "http://127.0.0.1:21500/login?x=1");
        let v = rewrite_location(&HeaderValue::from_static("/dash"), "http://127.0.0.1:21500");
        assert_eq!(v.to_str().unwrap(), "/dash");
    }

    #[test]
    fn set_cookie_secure_dropped_domain_rewritten() {
        let v = rewrite_set_cookie(&HeaderValue::from_static(
            "sid=abc; Path=/; HttpOnly; Secure; Domain=192.168.1.5; SameSite=None",
        ));
        assert_eq!(
            v.to_str().unwrap(),
            "sid=abc; Path=/; HttpOnly; Domain=127.0.0.1; SameSite=Lax"
        );
        let v = rewrite_set_cookie(&HeaderValue::from_static("t=1"));
        assert_eq!(v.to_str().unwrap(), "t=1");
    }

    #[test]
    fn upgrade_detection() {
        let req = Request::builder()
            .uri("http://127.0.0.1/ws")
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .body(())
            .unwrap();
        assert!(is_upgrade(&req));
        let req = Request::builder()
            .uri("http://127.0.0.1/")
            .header("connection", "keep-alive")
            .body(())
            .unwrap();
        assert!(!is_upgrade(&req));
    }

    #[test]
    fn hop_by_hop_not_copied() {
        let mut from = hyper::HeaderMap::new();
        from.insert(HOST, HeaderValue::from_static("127.0.0.1:21500"));
        from.insert(
            hyper::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );
        from.insert(hyper::header::COOKIE, HeaderValue::from_static("a=b"));
        let mut to = hyper::HeaderMap::new();
        copy_headers(&from, &mut to);
        assert!(to.get(HOST).is_none());
        assert!(to.get(hyper::header::CONNECTION).is_none());
        assert_eq!(to.get(hyper::header::COOKIE).unwrap(), "a=b");
    }

    /* ─────────────────────── end-to-end against a real TLS+WS upstream ────────────── */

    use http_body_util::Full;
    use rustls::pki_types::PrivateKeyDer;
    use rustls::ServerConfig;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio_rustls::TlsAcceptor;

    fn empty_body() -> BoxBody<Bytes, IoError> {
        Empty::<Bytes>::new().map_err(|e| match e {}).boxed()
    }

    /// HTTPS server (self-signed, like local-chat) with /redirect, /echo and /ws.
    async fn spawn_test_server() -> u16 {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let key = PrivateKeyDer::Pkcs8(ck.key_pair.serialize_der().into());
        let mut cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![ck.cert.into()], key)
            .unwrap();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let acceptor = acceptor.clone();
                let authority = format!("127.0.0.1:{port}");
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else { return };
                    let io = TokioIo::new(tls);
                    let svc = service_fn(move |req: Request<Incoming>| {
                        let authority = authority.clone();
                        async move { test_handle(req, authority).await }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(io, svc)
                        .with_upgrades()
                        .await;
                });
            }
        });
        port
    }

    async fn test_handle(
        mut req: Request<Incoming>,
        authority: String,
    ) -> Result<ProxyRes, hyper::Error> {
        match req.uri().path() {
            "/redirect" => Ok(Response::builder()
                .status(302)
                .header("location", format!("https://{authority}/target"))
                .header("set-cookie", "sid=abc; Path=/; HttpOnly; Secure")
                .body(empty_body())
                .unwrap()),
            "/echo" => {
                let body = req.collect().await.unwrap().to_bytes();
                Ok(Response::builder()
                    .status(200)
                    .body(Full::new(body).map_err(|e| match e {}).boxed())
                    .unwrap())
            }
            "/ws" => {
            let up = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                if let Ok(io) = up.await {
                    ws_echo(TokioIo::new(io)).await;
                }
            });
                Ok(Response::builder()
                    .status(101)
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade")
                    .header("sec-websocket-accept", "dummy")
                    .body(empty_body())
                    .unwrap())
            }
            _ => Ok(Response::builder()
                .status(404)
                .body(empty_body())
                .unwrap()),
        }
    }

    /// Read one masked text frame, reply with "echo:<payload>" unmasked.
    async fn ws_echo<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut s: S) {
        let mut hdr = [0u8; 2];
        if s.read_exact(&mut hdr).await.is_err() {
            return;
        }
        let masked = hdr[1] & 0x80 != 0;
        let len = (hdr[1] & 0x7f) as usize;
        let mut key = [0u8; 4];
        if masked && s.read_exact(&mut key).await.is_err() {
            return;
        }
        let mut payload = vec![0u8; len];
        if s.read_exact(&mut payload).await.is_err() {
            return;
        }
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= key[i % 4];
            }
        }
        let reply = format!("echo:{}", String::from_utf8_lossy(&payload));
        let mut out = vec![0x81u8, reply.len() as u8];
        out.extend_from_slice(reply.as_bytes());
        let _ = s.write_all(&out).await;
    }

    async fn run_proxy_e2e() {
        let upstream_port = spawn_test_server().await;
        let target = format!("https://127.0.0.1:{upstream_port}");
        let pp = start_proxy(&target).await.unwrap();

        /* ── GET /redirect: status, Location + Set-Cookie rewriting ── */
        let tcp = TcpStream::connect(("127.0.0.1", pp)).await.unwrap();
        let io = TokioIo::new(tcp);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(async move { let _ = conn.await; });
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://127.0.0.1:{pp}/redirect"))
            .header(HOST, format!("127.0.0.1:{pp}"))
            .body(Empty::<Bytes>::new().boxed())
            .unwrap();
        let res = sender.send_request(req).await.unwrap();
        assert_eq!(res.status(), 302);
        assert_eq!(
            res.headers().get("location").unwrap().to_str().unwrap(),
            format!("http://127.0.0.1:{pp}/target")
        );
        assert_eq!(
            res.headers().get(SET_COOKIE).unwrap().to_str().unwrap(),
            "sid=abc; Path=/; HttpOnly"
        );

        /* ── POST /echo: streamed body passthrough ── */
        let req = Request::builder()
            .method("POST")
            .uri(format!("http://127.0.0.1:{pp}/echo"))
            .header(HOST, format!("127.0.0.1:{pp}"))
            .body(Full::new(Bytes::from_static(b"hello-stream")).boxed())
            .unwrap();
        let res = sender.send_request(req).await.unwrap();
        assert_eq!(res.status(), 200);
        let body = res.collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"hello-stream");
        drop(sender);

        /* ── GET /ws: WebSocket upgrade + bidirectional frames ── */
        let tcp = TcpStream::connect(("127.0.0.1", pp)).await.unwrap();
        let mut io = BufReader::new(tcp);
        io.write_all(
            format!(
                "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{pp}\r\n\
                 Upgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 Sec-WebSocket-Version: 13\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let mut head = String::new();
        loop {
            let mut line = String::new();
            let n = io.read_line(&mut line).await.unwrap();
            assert!(n > 0, "ws handshake closed early");
            head.push_str(&line);
            if line.trim_end() == "" {
                break;
            }
        }
        assert!(head.contains(" 101 "), "handshake was: {head}");

        // masked client text frame "hi"
        let payload = b"hi";
        let key = [0x12u8, 0x34, 0x56, 0x78];
        let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&key);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        io.write_all(&frame).await.unwrap();

        let mut fh = [0u8; 2];
        io.read_exact(&mut fh).await.unwrap();
        assert_eq!(fh[0], 0x81, "expected FIN text frame");
        assert_eq!(fh[1] & 0x80, 0, "server frames must be unmasked");
        let mut rp = vec![0u8; (fh[1] & 0x7f) as usize];
        io.read_exact(&mut rp).await.unwrap();
        assert_eq!(&rp[..], b"echo:hi");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn proxy_end_to_end_http_ws() {
        match tokio::time::timeout(Duration::from_secs(15), run_proxy_e2e()).await {
            Ok(()) => {}
            Err(_) => panic!("proxy e2e test timed out"),
        }
    }
}

fn error_response(status: StatusCode, msg: &str) -> ProxyRes {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            http_body_util::Full::new(Bytes::from(msg.to_string()))
                .map_err(|e| match e {})
                .boxed(),
        )
        .unwrap()
}
