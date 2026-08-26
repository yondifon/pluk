//! A thin headless entry point, kept so a future headless deployment stays
//! possible. Nothing launches this today: the MCP server runs inside the
//! Tauri host process.

use std::sync::Arc;

use pluk_adapters::AdapterRegistry;
use pluk_server::ServerConfig;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let store = match pluk_store::Store::open_default() {
        Ok(store) => Arc::new(store),
        Err(error) => {
            eprintln!("cannot open the Pluk database: {error}");
            std::process::exit(1);
        }
    };

    // Per-service adapters register here as they are ported (R06–R14).
    let config = ServerConfig::new(store, Arc::new(AdapterRegistry::new()));

    let shutdown = tokio_util::sync::CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
                tokio::select! {
                    _ = ctrl_c => {},
                    _ = sigterm.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = ctrl_c.await;
            }
            shutdown.cancel();
        });
    }

    pluk_server::serve(config, shutdown).await
}
