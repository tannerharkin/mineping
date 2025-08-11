use crate::error::{ApiError, ApiResult};
use crate::minecraft;
use axum::extract::{Path, Query, ConnectInfo};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, Instant};
use tokio::sync::{RwLock, OnceCell, mpsc, Mutex};
use tracing::{info, warn};


// Global singletons
static STATS: OnceCell<Arc<Stats>> = OnceCell::const_new();
static CACHE: OnceCell<Arc<RwLock<HashMap<String, CachedResponse>>>> = OnceCell::const_new();
static TICKETS: OnceCell<Arc<RwLock<HashMap<String, TicketData>>>> = OnceCell::const_new();
static WORK_QUEUE: OnceCell<Arc<WorkQueue>> = OnceCell::const_new();
static QUEUE_SENDER: OnceCell<mpsc::UnboundedSender<WorkItem>> = OnceCell::const_new();

const CACHE_TTL_SECONDS: u64 = 60; // Cache for 1 minute
const WORKER_COUNT: usize = 10; // Number of background workers
const TICKET_TTL_SECONDS: u64 = 300; // Tickets expire after 5 minutes

#[derive(Clone)]
struct CachedResponse {
    data: minecraft::common::ServerInfo,
    timestamp: SystemTime,
}

#[derive(Clone)]
struct TicketData {
    status: TicketStatus,
    created_at: SystemTime,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status")]
pub enum TicketStatus {
    #[serde(rename = "pending")]
    Pending { position: usize },
    #[serde(rename = "processing")]
    Processing,
    #[serde(rename = "ready")]
    Ready { result: minecraft::common::ServerInfo },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Clone)]
struct WorkItem {
    ticket_id: String,
    host: String,
    port: u16,
}

struct WorkQueue {
    pending_tickets: RwLock<VecDeque<String>>,
    processing_count: AtomicUsize,
    total_queued: AtomicU64,
}

impl WorkQueue {
    fn new() -> Self {
        Self {
            pending_tickets: RwLock::new(VecDeque::new()),
            processing_count: AtomicUsize::new(0),
            total_queued: AtomicU64::new(0),
        }
    }

    async fn add_ticket(&self, ticket_id: String) -> usize {
        let mut queue = self.pending_tickets.write().await;
        queue.push_back(ticket_id);
        self.total_queued.fetch_add(1, Ordering::Relaxed);
        queue.len()
    }

    async fn get_position(&self, ticket_id: &str) -> Option<usize> {
        let queue = self.pending_tickets.read().await;
        queue.iter().position(|id| id == ticket_id).map(|pos| pos + 1)
    }

    async fn remove_ticket(&self, ticket_id: &str) {
        let mut queue = self.pending_tickets.write().await;
        queue.retain(|id| id != ticket_id);
    }

    fn start_processing(&self) {
        self.processing_count.fetch_add(1, Ordering::Relaxed);
    }

    fn finish_processing(&self) {
        self.processing_count.fetch_sub(1, Ordering::Relaxed);
    }

    async fn get_stats(&self) -> (usize, usize, u64) {
        let queue = self.pending_tickets.read().await;
        (
            queue.len(),
            self.processing_count.load(Ordering::Relaxed),
            self.total_queued.load(Ordering::Relaxed),
        )
    }
}

#[derive(Serialize)]
pub struct TicketResponse {
    ticket_id: String,
    #[serde(flatten)]
    status: TicketStatus,
    created_at: u64,
    age_seconds: u64,
}

#[derive(Serialize)]
pub struct TicketCreatedResponse {
    ticket_id: String,
    check_url: String,
}

async fn get_stats() -> &'static Arc<Stats> {
    STATS.get_or_init(|| async { Arc::new(Stats::new()) }).await
}

async fn get_cache() -> &'static Arc<RwLock<HashMap<String, CachedResponse>>> {
    CACHE.get_or_init(|| async { Arc::new(RwLock::new(HashMap::new())) }).await
}

async fn get_tickets() -> &'static Arc<RwLock<HashMap<String, TicketData>>> {
    TICKETS.get_or_init(|| async { Arc::new(RwLock::new(HashMap::new())) }).await
}

async fn get_work_queue() -> &'static Arc<WorkQueue> {
    WORK_QUEUE.get_or_init(|| async { Arc::new(WorkQueue::new()) }).await
}

async fn get_queue_sender() -> &'static mpsc::UnboundedSender<WorkItem> {
    QUEUE_SENDER.get_or_init(|| async {
        let (tx, rx) = mpsc::unbounded_channel();
        
        // Spawn worker tasks
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..WORKER_COUNT {
            let rx_clone = rx.clone();
            tokio::spawn(async move {
                worker_task(i, rx_clone).await;
            });
        }
        
        // Spawn cleanup task
        tokio::spawn(async {
            cleanup_task().await;
        });
        
        tx
    }).await
}

async fn cleanup_task() {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
    
    loop {
        interval.tick().await;
        
        let mut tickets = get_tickets().await.write().await;
        let queue = get_work_queue().await;
        let now = SystemTime::now();
        
        // Remove expired tickets
        let expired: Vec<String> = tickets
            .iter()
            .filter_map(|(id, data)| {
                if now.duration_since(data.created_at).unwrap_or_default().as_secs() > TICKET_TTL_SECONDS {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        
        for id in expired {
            tickets.remove(&id);
            queue.remove_ticket(&id).await;
        }
    }
}

async fn worker_task(id: usize, rx: Arc<Mutex<mpsc::UnboundedReceiver<WorkItem>>>) {
    info!("Worker {} started", id);
    
    loop {
        let item = {
            let mut rx_guard = rx.lock().await;
            rx_guard.recv().await
        };
        
        let Some(work) = item else {
            break;
        };
        
        let queue = get_work_queue().await;
        
        // Remove from pending queue and mark as processing
        queue.remove_ticket(&work.ticket_id).await;
        queue.start_processing();
        
        // Update status to processing
        {
            let mut tickets = get_tickets().await.write().await;
            if let Some(ticket_data) = tickets.get_mut(&work.ticket_id) {
                ticket_data.status = TicketStatus::Processing;
            }
        }
        
        // Perform the actual ping
        let start = Instant::now();
        let result = minecraft::ping_server(&work.host, work.port).await;
        let elapsed = start.elapsed().as_millis();
        
        // Mark processing as complete
        queue.finish_processing();
        
        // Update ticket with result
        let mut tickets = get_tickets().await.write().await;
        if let Some(ticket_data) = tickets.get_mut(&work.ticket_id) {
            match result {
                Ok(info) => {
                    info!("Worker {} completed ping for {}: success ({}ms)", id, work.host, elapsed);
                    
                    // Update cache
                    let cache_key = format!("{}:{}", work.host, work.port);
                    let cached = CachedResponse {
                        data: info.clone(),
                        timestamp: SystemTime::now(),
                    };
                    get_cache().await.write().await.insert(cache_key, cached);
                    
                    // Update stats
                    get_stats().await.record_request(true, Some(&info.protocol_used), info.query_enabled).await;
                    
                    // Update ticket
                    ticket_data.status = TicketStatus::Ready { result: info };
                }
                Err(e) => {
                    warn!("Worker {} failed ping for {}: {}", id, work.host, e);
                    get_stats().await.record_request(false, None, false).await;
                    
                    // Determine error message
                    let error_msg = if e.contains("timeout") || e.contains("Timeout") {
                        "Request timed out".to_string()
                    } else if e.contains("Connection failed") || e.contains("Connection refused") || e.contains("could not resolve") {
                        format!("Cannot reach {}", work.host)
                    } else if e.contains("All protocols failed") {
                        format!("{} does not appear to be a Minecraft server", work.host)
                    } else {
                        "Unable to communicate with server".to_string()
                    };
                    
                    ticket_data.status = TicketStatus::Error { message: error_msg };
                }
            }
        }
    }
}

fn generate_ticket_id(host: &str, port: u16, fresh: bool) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    host.hash(&mut hasher);
    port.hash(&mut hasher);
    fresh.hash(&mut hasher);
    
    // Add current minute to make tickets expire naturally
    let current_minute = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() / 60;
    current_minute.hash(&mut hasher);
    
    format!("{:016x}", hasher.finish())
}

struct Stats {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    start_time: SystemTime,
    protocol_counts: RwLock<ProtocolCounts>,
}

#[derive(Default)]
struct ProtocolCounts {
    modern_slp: u64,
    legacy_slp: u64,
    query_enabled: u64,
}

impl Stats {
    fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            start_time: SystemTime::now(),
            protocol_counts: RwLock::new(ProtocolCounts::default()),
        }
    }
    
    async fn record_request(&self, success: bool, protocol: Option<&str>, query_enabled: bool) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        
        if success {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
            
            if let Some(proto) = protocol {
                let mut counts = self.protocol_counts.write().await;
                match proto {
                    "modern_slp" => counts.modern_slp += 1,
                    "legacy_slp" | "legacy_slp_beta" => counts.legacy_slp += 1,
                    _ => {}
                }
                if query_enabled {
                    counts.query_enabled += 1;
                }
            }
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Serialize)]
pub struct StatsResponse {
    uptime_seconds: u64,
    total_requests: u64,
    successful_requests: u64,
    failed_requests: u64,
    success_rate: f64,
    requests_per_minute: f64,
    protocol_stats: ProtocolStats,
    queue_stats: QueueStats,
}

#[derive(Serialize)]
struct ProtocolStats {
    modern_slp_percentage: f64,
    legacy_slp_percentage: f64,
    query_enabled_percentage: f64,
}

#[derive(Serialize)]
struct QueueStats {
    pending_tickets: usize,
    processing_tickets: usize,
    total_queued: u64,
}

#[derive(Deserialize, Default)]
pub struct PingParams {
    #[serde(default)]
    fresh: bool,
}

pub async fn ping_with_port_str(
    Path((host, port_str)): Path<(String, String)>,
    Query(params): Query<PingParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> ApiResult<Json<TicketCreatedResponse>> {
    // Parse and validate port
    let port = match port_str.parse::<u32>() {
        Ok(p) if p > 0 && p <= 65535 => p as u16,
        _ => return Err(ApiError::InvalidPort),
    };
    
    info!("Received ping request from {} for {}:{}", addr.ip(), host, port);
    
    // Validate host
    if host.is_empty() || host.len() > 253 {
        return Err(ApiError::InvalidHost("Invalid hostname".to_string()));
    }
    
    let cache_key = format!("{}:{}", host, port);
    let ticket_id = generate_ticket_id(&host, port, params.fresh);
    
    // Check if we already have this ticket
    {
        let tickets = get_tickets().await.read().await;
        if let Some(ticket_data) = tickets.get(&ticket_id) {
            // Only return existing ticket if it's not expired
            if ticket_data.created_at.elapsed().unwrap_or_default().as_secs() < TICKET_TTL_SECONDS {
                return Ok(Json(TicketCreatedResponse {
                    ticket_id: ticket_id.clone(),
                    check_url: format!("/status/{}", ticket_id),
                }));
            }
        }
    }
    
    // Check cache unless fresh is requested
    if !params.fresh {
        let cache = get_cache().await.read().await;
        if let Some(cached) = cache.get(&cache_key) {
            if cached.timestamp.elapsed().unwrap_or_default().as_secs() < CACHE_TTL_SECONDS {
                // Create an instant ready ticket
                get_tickets().await.write().await.insert(
                    ticket_id.clone(),
                    TicketData {
                        status: TicketStatus::Ready { result: cached.data.clone() },
                        created_at: SystemTime::now(),
                    }
                );
                
                return Ok(Json(TicketCreatedResponse {
                    ticket_id: ticket_id.clone(),
                    check_url: format!("/status/{}", ticket_id),
                }));
            }
        }
    }
    
    // Add to queue and create pending ticket
    let queue = get_work_queue().await;
    let position = queue.add_ticket(ticket_id.clone()).await;
    
    get_tickets().await.write().await.insert(
        ticket_id.clone(),
        TicketData {
            status: TicketStatus::Pending { position },
            created_at: SystemTime::now(),
        }
    );
    
    // Queue the work
    let work_item = WorkItem {
        ticket_id: ticket_id.clone(),
        host,
        port,
    };
    
    if let Err(e) = get_queue_sender().await.send(work_item) {
        warn!("Failed to queue work: {}", e);
        // Remove the ticket we just added
        get_tickets().await.write().await.remove(&ticket_id);
        queue.remove_ticket(&ticket_id).await;
        return Err(ApiError::ConnectionFailed("Service temporarily unavailable".to_string()));
    }
    
    Ok(Json(TicketCreatedResponse {
        ticket_id: ticket_id.clone(),
        check_url: format!("/status/{}", ticket_id),
    }))
}

pub async fn ping(
    Path(host): Path<String>,
    query: Query<PingParams>,
    conn_info: ConnectInfo<SocketAddr>,
) -> ApiResult<Json<TicketCreatedResponse>> {
    ping_with_port_str(Path((host, minecraft::common::DEFAULT_PORT.to_string())), query, conn_info).await
}

pub async fn check_status(
    Path(ticket_id): Path<String>,
) -> ApiResult<Json<TicketResponse>> {
    let tickets = get_tickets().await.read().await;
    
    match tickets.get(&ticket_id) {
        Some(ticket_data) => {
            // Update position if pending
            let status = match &ticket_data.status {
                TicketStatus::Pending { .. } => {
                    let queue = get_work_queue().await;
                    if let Some(position) = queue.get_position(&ticket_id).await {
                        TicketStatus::Pending { position }
                    } else {
                        ticket_data.status.clone()
                    }
                }
                _ => ticket_data.status.clone(),
            };
            
            let created_at = ticket_data.created_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            let age_seconds = ticket_data.created_at
                .elapsed()
                .unwrap_or_default()
                .as_secs();
            
            Ok(Json(TicketResponse {
                ticket_id,
                status,
                created_at,
                age_seconds,
            }))
        }
        None => Err(ApiError::InvalidHost("Ticket not found or expired".to_string())),
    }
}

pub async fn teapot() -> impl IntoResponse {
    (
        axum::http::StatusCode::IM_A_TEAPOT,
        Json(serde_json::json!({
            "error": "I'm a teapot - This is a Minecraft server ping API",
            "usage": "GET /ping/{host}/{port} to get a ticket, then GET /status/{ticket_id} to check status",
            "endpoints": [
                "/ping/{host}/{port}",
                "/ping/{host}",
                "/status/{ticket_id}",
                "/health",
                "/stats"
            ]
        }))
    )
}

pub async fn health() -> Json<serde_json::Value> {
    let queue = get_work_queue().await;
    let (pending, processing, total) = queue.get_stats().await;
    
    Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "queue": {
            "pending": pending,
            "processing": processing,
            "total_queued": total,
        }
    }))
}

pub async fn stats() -> Json<StatsResponse> {
    let stats = get_stats().await;
    let total = stats.total_requests.load(Ordering::Relaxed);
    let successful = stats.successful_requests.load(Ordering::Relaxed);
    let failed = stats.failed_requests.load(Ordering::Relaxed);
    
    let uptime = stats.start_time.elapsed().unwrap_or_default().as_secs();
    let success_rate = if total > 0 {
        (successful as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    
    let rpm = if uptime > 0 {
        (total as f64 / uptime as f64) * 60.0
    } else {
        0.0
    };
    
    let counts = stats.protocol_counts.read().await;
    let protocol_stats = ProtocolStats {
        modern_slp_percentage: if successful > 0 {
            (counts.modern_slp as f64 / successful as f64) * 100.0
        } else {
            0.0
        },
        legacy_slp_percentage: if successful > 0 {
            (counts.legacy_slp as f64 / successful as f64) * 100.0
        } else {
            0.0
        },
        query_enabled_percentage: if successful > 0 {
            (counts.query_enabled as f64 / successful as f64) * 100.0
        } else {
            0.0
        },
    };
    
    let queue = get_work_queue().await;
    let (pending, processing, total_queued) = queue.get_stats().await;
    let queue_stats = QueueStats {
        pending_tickets: pending,
        processing_tickets: processing,
        total_queued,
    };
    
    Json(StatsResponse {
        uptime_seconds: uptime,
        total_requests: total,
        successful_requests: successful,
        failed_requests: failed,
        success_rate,
        requests_per_minute: rpm,
        protocol_stats,
        queue_stats,
    })
}