pub mod common;
pub mod legacy_slp;
pub mod modern_slp;
pub mod query;

use crate::minecraft::common::*;
use std::net::ToSocketAddrs;
use tokio::time::{timeout, Duration};
use trust_dns_resolver::TokioAsyncResolver;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};

pub async fn ping_server(host: &str, port: u16) -> Result<ServerInfo, String> {
    // Resolve the server address (check for SRV records)
    let (resolved_host, resolved_port, resolved_from, hostname) = resolve_server(host, port).await?;
    
    // Try modern SLP first
    match modern_slp::ping(&resolved_host, resolved_port, &hostname).await {
        Ok(mut info) => {
            info.resolved_from = resolved_from;
            
            // Try query in parallel for additional info (but don't fail if it doesn't work)
            if let Ok(query_timeout) = timeout(Duration::from_millis(1000), 
                query::query(&resolved_host, resolved_port)).await {
                if query_timeout.is_ok() {
                    info.query_enabled = true;
                }
            }
            
            return Ok(info);
        }
        Err(modern_err) => {
            // Try legacy SLP as fallback
            match legacy_slp::ping(&resolved_host, resolved_port).await {
                Ok(mut info) => {
                    info.resolved_from = resolved_from;
                    return Ok(info);
                }
                Err(legacy_err) => {
                    return Err(format!(
                        "All protocols failed. Modern: {}. Legacy: {}", 
                        modern_err, legacy_err
                    ));
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
    
    // Try SRV record lookup for _minecraft._tcp.domain
    let srv_domain = format!("_minecraft._tcp.{}", host);
    
    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    
    // Try SRV lookup
    if let Ok(srv_lookup) = resolver.srv_lookup(&srv_domain).await {
        if let Some(srv) = srv_lookup.iter().next() {
            let target = srv.target().to_string().trim_end_matches('.').to_string();
            
            // Resolve the SRV target to an IP
            if let Ok(ip_lookup) = resolver.lookup_ip(&target).await {
                if let Some(ip) = ip_lookup.iter().next() {
                    return Ok((
                        ip.to_string(), 
                        srv.port(), 
                        "srv".to_string(),
                        target  // Use SRV target as hostname for handshake
                    ));
                }
            }
        }
    }
    
    // Fall back to A/AAAA record lookup
    match resolver.lookup_ip(host).await {
        Ok(lookup) => {
            if let Some(ip) = lookup.iter().next() {
                Ok((ip.to_string(), port, "dns".to_string(), host.to_string()))
            } else {
                Err(format!("No IP addresses found for {}", host))
            }
        }
        Err(_) => {
            // Last resort: try system resolver
            match (host, port).to_socket_addrs() {
                Ok(mut addrs) => {
                    if let Some(addr) = addrs.next() {
                        Ok((addr.ip().to_string(), port, "system".to_string(), host.to_string()))
                    } else {
                        Err(format!("Could not resolve {}", host))
                    }
                }
                Err(e) => Err(format!("Failed to resolve {}: {}", host, e))
            }
        }
    }
}