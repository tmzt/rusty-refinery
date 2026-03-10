use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

pub const DEFAULT_SOCKET_PATH: &str = "/tmp/crk.sock";

/// Run as a daemon listening on a Unix domain socket.
/// For each connection, calls the provided async factory to serve it.
pub async fn listen<F, Fut>(socket_path: &str, serve_fn: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: Fn(Box<dyn AsyncRead + Send + Unpin + 'static>, Box<dyn AsyncWrite + Send + Unpin + 'static>) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
        + Send
        + 'static,
{
    // Remove stale socket
    if Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!(socket_path, "daemon listening on UDS");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let (reader, writer) = stream.into_split();
                let reader: Box<dyn AsyncRead + Send + Unpin + 'static> = Box::new(reader);
                let writer: Box<dyn AsyncWrite + Send + Unpin + 'static> = Box::new(writer);
                let fut = serve_fn(reader, writer);
                tokio::spawn(async move {
                    if let Err(e) = fut.await {
                        error!(%e, "session error");
                    }
                });
            }
            Err(e) => {
                error!(%e, "accept failed");
            }
        }
    }
}

/// Run as a proxy: connect to the daemon's UDS and bridge stdio <-> socket.
pub async fn proxy(socket_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (mut sock_read, mut sock_write) = stream.into_split();

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let to_sock = tokio::io::copy(&mut stdin, &mut sock_write);
    let from_sock = tokio::io::copy(&mut sock_read, &mut stdout);

    tokio::select! {
        result = to_sock => {
            result?;
        }
        result = from_sock => {
            result?;
        }
    }

    Ok(())
}
