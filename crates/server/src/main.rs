//! Knot Language Server — entry point.
//!
//! Parse CLI args (`--stdio`), initialize logging, and run the server.

use knot_server::KnotServer;

#[tokio::main]
async fn main() {
    // Initialize tracing — MUST write to stderr, since stdout is the LSP transport
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("knot_server=info".parse().unwrap()),
        )
        .init();

    // Check for --stdio flag
    let args: Vec<String> = std::env::args().collect();
    if !args.contains(&"--stdio".to_string()) {
        eprintln!("Knot Language Server requires --stdio flag");
        std::process::exit(1);
    }

    tracing::info!("Starting Knot Language Server");

    // Set up panic hook for crash recovery
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("Panic in language server: {:?}", info);
    }));

    // Run the server
    let server = KnotServer::new();
    if let Err(e) = server.run().await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}
