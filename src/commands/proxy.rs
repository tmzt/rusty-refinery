use tracing::info;

pub async fn run(socket: &str) -> Result<(), Box<dyn std::error::Error>> {
    info!(socket_path = %socket, "proxy mode: connecting to daemon");
    crate::proxy::proxy(socket).await?;
    Ok(())
}
