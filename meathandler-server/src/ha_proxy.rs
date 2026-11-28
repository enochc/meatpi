//! HTTPS front for Home Assistant on its own port (default :8444).
//!
//! TLS ends here with the same certificates as :8443 (the No-IP cert for
//! meathandler.ddns.net, the local-CA cert by IP). The decrypted bytes are then
//! piped as-is to Home Assistant's plain HTTP port on the LAN, so HTTP and the
//! frontend's websocket both pass through untouched. Home Assistant does its
//! own login; this port adds no auth of its own.
//!
//! No X-Forwarded-For is added, so Home Assistant sees every client as this
//! Pi and needs no `trusted_proxies` entry. Keep Home Assistant's `ip_ban`
//! off, or one bad login would lock everyone out through this proxy.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

pub async fn run(listen: SocketAddr, upstream: String, tls: Arc<ServerConfig>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    let acceptor = TlsAcceptor::from(tls);
    tracing::info!(%listen, %upstream, "home assistant proxy listening");
    loop {
        let (client, peer) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("ha proxy accept: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let upstream = upstream.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(client, &acceptor, &upstream).await {
                tracing::debug!(%peer, "ha proxy: {e:#}");
            }
        });
    }
}

async fn serve(client: TcpStream, acceptor: &TlsAcceptor, upstream: &str) -> anyhow::Result<()> {
    let mut tls = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(client)).await??;
    let mut up = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(upstream)).await??;
    let _ = up.set_nodelay(true);
    tokio::io::copy_bidirectional(&mut tls, &mut up).await?;
    Ok(())
}
