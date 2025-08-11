use crate::minecraft::common::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

pub async fn ping(host: &str, port: u16) -> Result<ServerInfo, String> {
    let timeout_duration = Duration::from_millis(TIMEOUT_MS);
    
    // Connect with timeout
    let mut stream = timeout(timeout_duration, TcpStream::connect((host, port)))
        .await
        .map_err(|_| "Connection timeout".to_string())?
        .map_err(|e| format!("Connection failed: {}", e))?;
    
    // Send legacy ping packet (0xFE 0x01)
    stream.write_all(&[0xFE, 0x01]).await
        .map_err(|e| format!("Failed to send ping: {}", e))?;
    stream.flush().await.map_err(|e| e.to_string())?;
    
    // Read response
    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];
    
    loop {
        match timeout(Duration::from_millis(1000), stream.read(&mut buffer)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => response.extend_from_slice(&buffer[..n]),
            Ok(Err(e)) => return Err(format!("Read error: {}", e)),
            Err(_) => break, // Timeout, assume we got all data
        }
        
        if response.len() > 4096 {
            return Err("Response too large".to_string());
        }
    }
    
    if response.is_empty() {
        return Err("No response received".to_string());
    }
    
    // Parse response
    if response[0] != 0xFF {
        return Err(format!("Invalid response packet ID: 0x{:02x}", response[0]));
    }
    
    // Skip packet ID (0xFF) and length (2 bytes)
    if response.len() < 3 {
        return Err("Response too short".to_string());
    }
    
    // Read UTF-16BE string
    let string_start = 3;
    let string_bytes = &response[string_start..];
    
    // Convert UTF-16BE to String
    let mut chars = Vec::new();
    let mut i = 0;
    while i + 1 < string_bytes.len() {
        let high = string_bytes[i] as u16;
        let low = string_bytes[i + 1] as u16;
        let ch = (high << 8) | low;
        chars.push(ch);
        i += 2;
    }
    
    let response_str: String = String::from_utf16(&chars)
        .map_err(|_| "Invalid UTF-16".to_string())?;
    
    // Parse the response string
    if response_str.starts_with("§1\0") {
        // 1.4+ format
        let parts: Vec<&str> = response_str[3..].split('\0').collect();
        
        if parts.len() >= 5 {
            Ok(ServerInfo {
                online: true,
                host: host.to_string(),
                port,
                resolved_from: "direct".to_string(),
                version: Some(VersionInfo {
                    name: parts[1].to_string(),
                    protocol: parts[0].parse().unwrap_or(0),
                }),
                players: Some(PlayersInfo {
                    online: parts[3].parse().unwrap_or(0),
                    max: parts[4].parse().unwrap_or(0),
                    sample: None,
                }),
                description: Some(parts[2].to_string()),
                favicon: None,
                latency_ms: None,
                query_enabled: false,
                protocol_used: "legacy_slp".to_string(),
            })
        } else {
            Err("Invalid legacy response format".to_string())
        }
    } else {
        // Beta 1.8 to 1.3 format
        let parts: Vec<&str> = response_str.split('§').collect();
        
        if parts.len() >= 3 {
            Ok(ServerInfo {
                online: true,
                host: host.to_string(),
                port,
                resolved_from: "direct".to_string(),
                version: None,
                players: Some(PlayersInfo {
                    online: parts[1].parse().unwrap_or(0),
                    max: parts[2].parse().unwrap_or(0),
                    sample: None,
                }),
                description: Some(parts[0].to_string()),
                favicon: None,
                latency_ms: None,
                query_enabled: false,
                protocol_used: "legacy_slp_beta".to_string(),
            })
        } else {
            Err("Invalid legacy beta response format".to_string())
        }
    }
}