mod api;
mod error;
mod minecraft;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber;

fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    // Create web server runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(16)
        .thread_name("mineping-web")
        .max_blocking_threads(256)
        .enable_all()
        .build()
        .expect("Failed to create runtime");

    runtime.block_on(async {
        run_server().await;
    });
}

async fn run_server() {
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

    // Get port from environment or use default
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse::<u16>()
        .expect("Invalid PORT");
    
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting mineping on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}