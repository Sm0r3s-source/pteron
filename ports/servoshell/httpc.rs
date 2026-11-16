//! One shared HTTP client for Plume (YouTube, subtitles, lyrics, feeds).
//!
//! Before, every request started a `curl` process: a new process, a new connection and a new TLS
//! handshake each time. Here one client keeps connections open (and speaks HTTP/2 to YouTube), so
//! a request costs one round trip instead of five. `Err` means the request could not be made at
//! all (no connection, TLS refused, timeout): callers then fall back to curl, which uses the
//! system certificate store and proxy settings. An HTTP status such as 403 is an `Ok` answer.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

type Connector = hyper_rustls::HttpsConnector<HttpConnector>;

pub(crate) struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    /// Stop reading after this many bytes (the rest is dropped).
    pub max_body: usize,
    /// Follow up to five redirects.
    pub follow: bool,
    /// When not empty, redirects may only go to this host or its subdomains.
    pub stay_in: &'a str,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str, timeout: Duration) -> Self {
        Request { method: "GET", url, headers: &[], body: None, timeout, max_body: 4_000_000, follow: false, stay_in: "" }
    }
}

pub(crate) struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

struct Shared {
    runtime: tokio::runtime::Runtime,
    client: Client<Connector, Full<Bytes>>,
}

fn shared() -> Result<&'static Shared, String> {
    static SHARED: OnceLock<Result<Shared, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("pteron-http")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let connector = HttpsConnectorBuilder::new()
                .with_provider_and_webpki_roots(provider)
                .map_err(|e| e.to_string())?
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build();
            let client = Client::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(60))
                .pool_max_idle_per_host(6)
                .build(connector);
            Ok(Shared { runtime, client })
        })
        .as_ref()
        .map_err(|e| e.clone())
}

pub(crate) fn send(request: &Request) -> Result<Response, String> {
    let shared = shared()?;
    shared
        .runtime
        .block_on(async { tokio::time::timeout(request.timeout, run(&shared.client, request)).await })
        .map_err(|_| "timed out".to_string())?
}

async fn run(client: &Client<Connector, Full<Bytes>>, request: &Request<'_>) -> Result<Response, String> {
    let mut url = request.url.to_string();
    for _ in 0..6 {
        let mut builder = hyper::Request::builder().method(request.method).uri(&url);
        for (name, value) in request.headers {
            builder = builder.header(*name, *value);
        }
        let body = Full::new(Bytes::from(request.body.clone().unwrap_or_default()));
        let hyper_request = builder.body(body).map_err(|e| e.to_string())?;
        let mut response = client.request(hyper_request).await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        if request.follow && matches!(status, 301 | 302 | 303 | 307 | 308) {
            let next = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .and_then(|l| url::Url::parse(&url).ok()?.join(l).ok());
            match next {
                // Never leave https for http on the way.
                Some(next)
                    if (next.scheme() == "https" || !url.starts_with("https:"))
                        && (request.stay_in.is_empty()
                            || next.host_str().is_some_and(|h| {
                                h == request.stay_in || h.ends_with(&format!(".{}", request.stay_in))
                            })) =>
                {
                    url = next.to_string();
                    continue;
                },
                _ => return Err("bad redirect".into()),
            }
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            let frame = frame.map_err(|e| e.to_string())?;
            if let Some(data) = frame.data_ref() {
                let room = request.max_body.saturating_sub(body.len());
                body.extend_from_slice(&data[..data.len().min(room)]);
                if body.len() >= request.max_body {
                    break;
                }
            }
        }
        return Ok(Response { status, content_type, body });
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A tiny local server: answers every request on one connection, counting connections.
    fn server(replies: Vec<String>) -> (String, std::sync::mpsc::Receiver<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut connections = 0;
            let mut replies = replies.into_iter();
            while let Some(reply) = replies.next() {
                let (mut stream, _) = listener.accept().unwrap();
                connections += 1;
                let _ = tx.send(connections);
                let mut next = Some(reply);
                loop {
                    let mut buf = [0u8; 4096];
                    if stream.read(&mut buf).unwrap_or(0) == 0 {
                        break;
                    }
                    let Some(r) = next.take().or_else(|| replies.next()) else { break };
                    let _ = stream.write_all(r.as_bytes());
                }
            }
        });
        (address, rx)
    }

    fn ok(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn get_post_and_a_reused_connection() {
        let (address, connections) = server(vec![ok("one"), ok("two")]);
        let a = send(&Request::get(&format!("{address}/a"), Duration::from_secs(5))).unwrap();
        assert_eq!((a.status, a.body.as_slice(), a.content_type.as_str()), (200, &b"one"[..], "text/plain"));
        let url_b = format!("{address}/b");
        let mut post = Request::get(&url_b, Duration::from_secs(5));
        post.method = "POST";
        post.body = Some(b"{}".to_vec());
        post.headers = &[("content-type", "application/json")];
        let b = send(&post).unwrap();
        assert_eq!(b.body, b"two");
        // both requests used the same connection: only one was ever accepted
        assert_eq!(connections.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert!(connections.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn status_is_an_answer_but_a_dead_address_is_an_error() {
        let (address, _rx) = server(vec!["HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".into()]);
        assert_eq!(send(&Request::get(&format!("{address}/x"), Duration::from_secs(5))).unwrap().status, 403);
        assert!(send(&Request::get("http://127.0.0.1:1/", Duration::from_secs(2))).is_err());
    }

    #[test]
    fn redirects_may_be_kept_on_one_site() {
        let (address, _rx) = server(vec!["HTTP/1.1 302 Found\r\nLocation: http://example.invalid/x\r\nContent-Length: 0\r\n\r\n".into()]);
        let url = format!("{address}/start");
        let mut r = Request::get(&url, Duration::from_secs(5));
        (r.follow, r.stay_in) = (true, "googlevideo.com");
        assert!(send(&r).is_err());
    }

    #[test]
    fn redirects_and_the_size_limit() {
        let (address, _rx) = server(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n".into(),
            ok("0123456789"),
        ]);
        let url_s = format!("{address}/start");
        let mut r = Request::get(&url_s, Duration::from_secs(5));
        r.follow = true;
        r.max_body = 4;
        let got = send(&r).unwrap();
        assert_eq!((got.status, got.body.as_slice()), (200, &b"0123"[..]));
    }
}
