mod api;
mod error;
mod minecraft;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize logging with a simple format
    tracing_subscriber::fmt()
        .with_target(false)
        .init();

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
    info!("Starting MinePing API on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}