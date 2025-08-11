pub mod common;
pub mod legacy_slp;
pub mod modern_slp;
pub mod query;

use crate::minecraft::common::*;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio::sync::OnceCell;
use trust_dns_resolver::TokioAsyncResolver;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};

// Global DNS resolver (but no connection limiter - that was the bottleneck!)
static DNS_RESOLVER: OnceCell<Arc<TokioAsyncResolver>> = OnceCell::const_new();

async fn get_resolver() -> &'static Arc<TokioAsyncResolver> {
    DNS_RESOLVER.get_or_init(|| async {
        let mut opts = ResolverOpts::default();
        opts.num_concurrent_reqs = 200;  // Allow many concurrent DNS queries
        opts.cache_size = 4096;  // Large cache to reduce DNS lookups
        opts.use_hosts_file = false;
        opts.positive_min_ttl = Some(Duration::from_secs(300)); // Cache for 5 minutes
        opts.negative_min_ttl = Some(Duration::from_secs(30));  // Cache failures for 30s
        opts.timeout = Duration::from_millis(1000);
        
        Arc::new(TokioAsyncResolver::tokio(
            ResolverConfig::default(),
            opts
        ))
    }).await
}

pub async fn ping_server(host: &str, port: u16) -> Result<ServerInfo, String> {
    // Resolve the server address first (no semaphore!)
    let (resolved_host, resolved_port, resolved_from, hostname) = resolve_server(host, port).await?;
    
    // Try modern SLP first
    let modern_timeout = Duration::from_millis(2000);
    match timeout(modern_timeout, modern_slp::ping(&resolved_host, resolved_port, &hostname)).await {
        Ok(Ok(mut info)) => {
            info.resolved_from = resolved_from;
            
            // Try query quickly
            let query_timeout = Duration::from_millis(200);
            if timeout(query_timeout, query::query(&resolved_host, resolved_port))
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false) {
                info.query_enabled = true;
            }
            
            return Ok(info);
        }
        Ok(Err(modern_err)) => {
            // Try legacy as fallback
            let legacy_timeout = Duration::from_millis(1500);
            match timeout(legacy_timeout, legacy_slp::ping(&resolved_host, resolved_port)).await {
                Ok(Ok(mut info)) => {
                    info.resolved_from = resolved_from;
                    return Ok(info);
                }
                Ok(Err(legacy_err)) => {
                    return Err(format!(
                        "All protocols failed. Modern: {}. Legacy: {}", 
                        modern_err, legacy_err
                    ));
                }
                Err(_) => {
                    return Err("Legacy protocol timeout".to_string());
                }
            }
        }
        Err(_) => {
            // Modern timeout, try legacy quickly
            let legacy_timeout = Duration::from_millis(1000);
            match timeout(legacy_timeout, legacy_slp::ping(&resolved_host, resolved_port)).await {
                Ok(Ok(mut info)) => {
                    info.resolved_from = resolved_from;
                    return Ok(info);
                }
                Ok(Err(e)) => {
                    return Err(format!("Timeout then legacy failed: {}", e));
                }
                Err(_) => {
                    return Err("All protocols timed out".to_string());
                }
            }
        }
    }
}

async fn resolve_server(host: &str, port: u16) -> Result<(String, u16, String, String), String> {
    // Check if it's already an IP address
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok((host.to_string(), port, "direct".to_string(), host.to_string()));
    }
    
    let resolver = get_resolver().await;
    
    // Try SRV record lookup
    let srv_domain = format!("_minecraft._tcp.{}", host);
    
    let srv_timeout = Duration::from_millis(500);
    if let Ok(srv_result) = timeout(srv_timeout, resolver.srv_lookup(&srv_domain)).await {
        if let Ok(srv_lookup) = srv_result {
            if let Some(srv) = srv_lookup.iter().next() {
                let target = srv.target().to_string().trim_end_matches('.').to_string();
                
                // Resolve the SRV target to an IP
                let ip_timeout = Duration::from_millis(500);
                if let Ok(ip_result) = timeout(ip_timeout, resolver.lookup_ip(&target)).await {
                    if let Ok(ip_lookup) = ip_result {
                        if let Some(ip) = ip_lookup.iter().next() {
                            return Ok((
                                ip.to_string(), 
                                srv.port(), 
                                "srv".to_string(),
                                target
                            ));
                        }
                    }
                }
            }
        }
    }
    
    // Fall back to A/AAAA record lookup
    let dns_timeout = Duration::from_millis(1000);
    match timeout(dns_timeout, resolver.lookup_ip(host)).await {
        Ok(Ok(lookup)) => {
            if let Some(ip) = lookup.iter().next() {
                Ok((ip.to_string(), port, "dns".to_string(), host.to_string()))
            } else {
                Err(format!("No IP addresses found for {}", host))
            }
        }
        Ok(Err(_)) | Err(_) => {
            // Last resort: try system resolver
            let system_timeout = Duration::from_millis(500);
            match timeout(system_timeout, tokio::task::spawn_blocking({
                let host = host.to_string();
                move || (host.as_str(), port).to_socket_addrs()
            })).await {
                Ok(Ok(Ok(mut addrs))) => {
                    if let Some(addr) = addrs.next() {
                        Ok((addr.ip().to_string(), port, "system".to_string(), host.to_string()))
                    } else {
                        Err(format!("Could not resolve {}", host))
                    }
                }
                _ => Err(format!("Failed to resolve {}: timeout or error", host))
            }
        }
    }
}