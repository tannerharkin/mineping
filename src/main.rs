mod api;
mod error;
mod minecraft;
mod tui;

use axum::{routing::get, Router};
use clap::Parser;
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::tui::{TuiState, init_tui_logging};

#[derive(Parser, Debug)]
#[command(name = "mineping")]
#[command(about = "REST API for querying Minecraft server status", long_about = None)]
struct Args {
    /// Port to listen on
    #[arg(short, long, env = "PORT", default_value_t = 3000)]
    port: u16,

    /// Disable interactive terminal UI
    #[arg(long, env = "DISABLE_TUI")]
    no_tui: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Number of worker threads for the web server
    #[arg(long, env = "WEB_WORKERS", default_value_t = 16)]
    web_workers: usize,

    /// Maximum number of blocking threads
    #[arg(long, env = "MAX_BLOCKING", default_value_t = 256)]
    max_blocking: usize,
}

fn main() {
    let args = Args::parse();

    // Create web server runtime with configured thread counts
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.web_workers)
        .thread_name("mineping-web")
        .max_blocking_threads(args.max_blocking)
        .enable_all()
        .build()
        .expect("Failed to create runtime");

    runtime.block_on(async {
        run_server(args).await;
    });
}

async fn run_server(args: Args) {
    // Initialize logging based on TUI mode
    if args.no_tui || !atty::is(atty::Stream::Stdout) {
        // Standard logging to console
        tracing_subscriber::fmt()
            .with_target(false)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&args.log_level))
            )
            .init();
        
        info!("Starting mineping in standard mode");
    } else {
        // TUI mode - logs will be displayed in the TUI
        info!("Starting mineping in interactive mode (use --no-tui to disable)");
    }
    
    // Initialize workers before starting server
    api::init().await;
    
    // Build the router
    let app = Router::new()
        .route("/ping/{host}/{port}", get(api::ping_with_port_str))
        .route("/ping/{host}", get(api::ping))
        .route("/status/{ticket_id}", get(api::check_status))
        .route("/health", get(api::health))
        .route("/stats", get(api::stats))
        .route("/", get(api::teapot))
        .fallback(api::teapot)
        .layer(
            ServiceBuilder::new()
                .layer(CorsLayer::permissive())
                .into_inner()
        );

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    
    if args.no_tui || !atty::is(atty::Stream::Stdout) {
        // Standard mode
        info!("Starting mineping on {}", addr);
        
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        
        // Setup graceful shutdown for standard mode
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap();
        });
        
        // Wait for server to finish
        server_handle.await.unwrap();
    } else {
        // TUI mode
        let tui_state = TuiState::new();
        
        // Initialize TUI logging
        init_tui_logging(&tui_state);
        
        info!("Starting mineping on {} with interactive UI", addr);
        
        // Get shutdown signal for TUI
        let should_quit = tui_state.should_quit();
        
        // Spawn the web server in the background
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async move {
                    // Wait for either TUI quit or Ctrl+C
                    tokio::select! {
                        _ = shutdown_signal() => {
                            // Ctrl+C pressed - signal TUI to quit
                            *should_quit.write().await = true;
                        }
                        _ = wait_for_quit(should_quit.clone()) => {
                            // TUI quit - we're done
                        }
                    }
                })
                .await
                .unwrap();
        });
        
        // Run the TUI (this blocks until user quits)
        if let Err(e) = tui_state.run().await {
            eprintln!("TUI error: {}", e);
        }
        
        // Wait for server to finish graceful shutdown
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            server_handle
        ).await;
    }
    
    info!("Mineping shutdown complete");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn wait_for_quit(should_quit: std::sync::Arc<tokio::sync::RwLock<bool>>) {
    loop {
        if *should_quit.read().await {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}