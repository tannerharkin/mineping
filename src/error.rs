use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    InvalidHost(String),
    InvalidPort,
    ConnectionFailed(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::InvalidHost(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::InvalidPort => (StatusCode::BAD_REQUEST, "Invalid port number".to_string()),
            ApiError::ConnectionFailed(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
        };

        let body = json!({
            "error": message,
            "online": false
        });

        (status, axum::Json(body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;