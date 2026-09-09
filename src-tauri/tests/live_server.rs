//! Smoke test of the reverse proxy against a REAL local-chat server.
//! Requires: https://127.0.0.1:6767 reachable. Run with:
//!   cargo test --test live_server -- --ignored

use hyper::body::Bytes;
use hyper::Request;
use mellow_client_lib::proxy::start_proxy;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const TARGET: &str = "https://127.0.0.1:6767";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_http_and_ws() {
    if tokio::net::TcpStream::connect("127.0.0.1:6767").await.is_err() {
        eprintln!("skipping: local-chat not running on {TARGET}");
        return;
    }
    let pp = start_proxy(TARGET).await.unwrap();

    // GET / must return the app HTML with 200.
    let tcp = TcpStream::connect(("127.0.0.1", pp)).await.unwrap();
    let io = hyper_util::rt::TokioIo::new(tcp);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(async move { let _ = conn.await; });
    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("host", format!("127.0.0.1:{pp}"))
        .body(http_body_util::Empty::<Bytes>::new())
        .unwrap();
    let res = sender.send_request(req).await.unwrap();
    assert_eq!(res.status(), 200, "GET / via proxy");
    let body = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&body).to_lowercase();
    assert!(html.contains("<html") || html.contains("<!doctype"), "html body");

    // WebSocket upgrade must produce a 101.
    let tcp = TcpStream::connect(("127.0.0.1", pp)).await.unwrap();
    let mut io = BufReader::new(tcp);
    io.write_all(
        format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{pp}\r\n\
             Upgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut line = String::new();
    io.read_line(&mut line).await.unwrap();
    assert!(line.contains(" 101 "), "ws handshake: {line}");
    println!("live proxy smoke OK (port {pp})");
}
