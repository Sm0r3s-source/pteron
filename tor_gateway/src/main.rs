//! Sirocco's Tor gateway. Started by Sirocco with the first private tab and stopped with the last.
//!
//! `sirocco-tor <state folder> <cache folder>`: listens for SOCKS5 on 127.0.0.1 (the site name is
//! sent along, so names are looked up inside Tor, not on this computer) and hands every connection to
//! an embedded Tor client. It reports on stdout: `LISTEN <address>`, then `READY` or `FAILED <why>`.
//! It exits when its standard input closes, so it never outlives Sirocco.

use std::io::{Read, Write};
use std::time::Duration;

use arti_client::TorClient;
use arti_client::config::TorClientConfigBuilder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type Client = std::sync::Arc<TorClient<tor_rtcompat::PreferredRuntime>>;

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut args = std::env::args().skip(1);
    let (Some(state), Some(cache)) = (args.next(), args.next()) else {
        eprintln!("usage: sirocco-tor <state folder> <cache folder>");
        std::process::exit(2);
    };
    // Gone when Sirocco is gone.
    std::thread::spawn(|| {
        let mut buffer = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buffer), Ok(n) if n > 0) {}
        std::process::exit(0);
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    runtime.block_on(run(state, cache));
}

async fn run(state: String, cache: String) {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)).await else {
        return say("FAILED no local port");
    };
    let Ok(address) = listener.local_addr() else {
        return say("FAILED no local address");
    };
    let (client_tx, client_rx) = tokio::sync::watch::channel(None::<Client>);
    say(&format!("LISTEN {address}"));
    tokio::spawn(async move {
        let config = match TorClientConfigBuilder::from_directories(state, cache).build() {
            Ok(config) => config,
            Err(error) => return say(&format!("FAILED {error}")),
        };
        match TorClient::builder().config(config).create_bootstrapped().await {
            Ok(client) => {
                let _ = client_tx.send(Some(client));
                say("READY");
            },
            Err(error) => say(&format!("FAILED {error}")),
        }
    });
    loop {
        let Ok((socket, _)) = listener.accept().await else { continue };
        let client_rx = client_rx.clone();
        tokio::spawn(async move {
            let _ = serve(socket, client_rx).await;
        });
    }
}

/// One SOCKS5 connection from a private tab.
async fn serve(
    mut socket: TcpStream,
    mut client_rx: tokio::sync::watch::Receiver<Option<Client>>,
) -> std::io::Result<()> {
    let bad = |text: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, text.to_string());
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await?;
    if request[0] != 5 || request[1] != 1 {
        socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(bad("only CONNECT"));
    }
    let host = match request[3] {
        3 => {
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| bad("bad name"))?
        },
        1 => {
            let mut ip = [0u8; 4];
            socket.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        },
        4 => {
            let mut ip = [0u8; 16];
            socket.read_exact(&mut ip).await?;
            std::net::Ipv6Addr::from(ip).to_string()
        },
        _ => return Err(bad("bad address type")),
    };
    let mut port = [0u8; 2];
    socket.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    // Wait for Tor (at most two minutes); if it never comes up the connection fails closed.
    let client = match tokio::time::timeout(Duration::from_secs(120), client_rx.wait_for(|client| client.is_some())).await {
        Ok(Ok(client)) => client.clone(),
        _ => None,
    };
    let Some(client) = client else {
        socket.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(bad("Tor is not ready"));
    };
    let mut stream = match client.connect((host.as_str(), port)).await {
        Ok(stream) => stream,
        Err(_) => {
            socket.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(bad("Tor could not connect"));
        },
    };
    socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
    Ok(())
}

