//! Knot Language Server — entry point.
//!
//! Parse CLI args (`--stdio`), initialize logging, and run the server.

use knot_server::KnotServer;

#[tokio::main]
async fn main() {
    // Set up panic hook FIRST — write to both stderr and a crash log file.
    // This ensures we can see panics even when stderr is consumed by the LSP transport.
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC in Knot Language Server: {:?}", info);
        eprintln!("{}", msg);
        // Also write to a crash log file so the user can find it
        if let Ok(dir) = std::env::temp_dir().into_os_string().into_string() {
            let crash_path = format!("{}/knot-crash.log", dir.trim_end_matches('/'));
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&crash_path)
            {
                use std::io::Write;
                let _ = writeln!(f, "[{}] {}", simple_timestamp(), msg);
            }
        }
    }));

    // Initialize tracing — write to BOTH stderr and a log file.
    // The log file lives in the system temp directory and persists across runs.
    let log_dir = std::env::temp_dir();
    let log_path = log_dir.join("knot-server.log");

    // Try to create the log file. If it fails, fall back to stderr-only.
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // Fresh log each session
        .open(&log_path);

    let directive = "knot_server=debug".parse::<tracing_subscriber::filter::Directive>()
        .unwrap_or_else(|e| {
            eprintln!("Invalid tracing directive: {e}");
            "info".parse().unwrap()
        });

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(directive);

    match log_file {
        Ok(file) => {
            // Write to both stderr and the log file using layered subscribers
            use tracing_subscriber::layer::SubscriberExt;
            use tracing_subscriber::Layer;
            let stderr_layer = tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .with_filter(env_filter.clone());
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(file)
                .with_ansi(false)
                .with_filter(env_filter);
            let subscriber = tracing_subscriber::registry()
                .with(stderr_layer)
                .with(file_layer);
            tracing::subscriber::set_global_default(subscriber)
                .unwrap_or_else(|e| eprintln!("Failed to set tracing subscriber: {e}"));
        }
        Err(e) => {
            eprintln!("Cannot create log file at {:?}: {}", log_path, e);
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .with_env_filter(env_filter)
                .init();
        }
    }

    tracing::info!("Starting Knot Language Server v2.0.0");
    tracing::info!("Log file: {}", log_path.display());
    tracing::info!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);

    // Check for --stdio flag
    let args: Vec<String> = std::env::args().collect();
    if !args.contains(&"--stdio".to_string()) {
        eprintln!("Knot Language Server requires --stdio flag");
        std::process::exit(1);
    }

    // Run the server
    let server = KnotServer::new();
    if let Err(e) = server.run().await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }

    tracing::info!("Server exited normally");
}

/// Simple timestamp without requiring the `chrono` crate.
fn simple_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}s", duration.as_secs(), duration.subsec_millis())
}
