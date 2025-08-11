use crate::minecraft::common::*;
use std::collections::HashMap;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

const QUERY_MAGIC: [u8; 2] = [0xFE, 0xFD];
const HANDSHAKE_TYPE: u8 = 0x09;
const STAT_TYPE: u8 = 0x00;

pub async fn query(host: &str, port: u16) -> Result<QueryInfo, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await
        .map_err(|e| format!("Failed to bind socket: {}", e))?;
    
    socket.connect((host, port)).await
        .map_err(|e| format!("Failed to connect: {}", e))?;
    
    // Generate session ID
    let session_id = generate_session_id();
    
    // Perform handshake
    let challenge_token = handshake(&socket, session_id).await?;
    
    // Get full stats
    let stats = get_full_stats(&socket, session_id, challenge_token).await?;
    
    Ok(stats)
}

fn generate_session_id() -> i32 {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i32;
    timestamp & 0x0F0F0F0F
}

async fn handshake(socket: &UdpSocket, session_id: i32) -> Result<i32, String> {
    // Build handshake packet
    let mut packet = Vec::new();
    packet.extend_from_slice(&QUERY_MAGIC);
    packet.push(HANDSHAKE_TYPE);
    packet.extend_from_slice(&session_id.to_be_bytes());
    
    // Send handshake
    socket.send(&packet).await
        .map_err(|e| format!("Failed to send handshake: {}", e))?;
    
    // Receive response
    let mut buffer = [0u8; 1024];
    let size = timeout(Duration::from_millis(TIMEOUT_MS), socket.recv(&mut buffer))
        .await
        .map_err(|_| "Handshake timeout".to_string())?
        .map_err(|e| format!("Failed to receive handshake: {}", e))?;
    
    let response = &buffer[..size];
    
    // Parse response
    if response.len() < 5 || response[0] != HANDSHAKE_TYPE {
        return Err("Invalid handshake response".to_string());
    }
    
    // Extract challenge token (null-terminated string starting at byte 5)
    let token_str = extract_null_terminated_string(response, 5)?;
    let challenge_token = token_str.parse::<i32>()
        .map_err(|_| "Invalid challenge token".to_string())?;
    
    Ok(challenge_token)
}

async fn get_full_stats(socket: &UdpSocket, session_id: i32, challenge_token: i32) -> Result<QueryInfo, String> {
    // Build full stat request
    let mut packet = Vec::new();
    packet.extend_from_slice(&QUERY_MAGIC);
    packet.push(STAT_TYPE);
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.extend_from_slice(&challenge_token.to_be_bytes());
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Padding for full stats
    
    // Send request
    socket.send(&packet).await
        .map_err(|e| format!("Failed to send stat request: {}", e))?;
    
    // Receive response
    let mut buffer = [0u8; 4096];
    let size = timeout(Duration::from_millis(TIMEOUT_MS), socket.recv(&mut buffer))
        .await
        .map_err(|_| "Stats timeout".to_string())?
        .map_err(|e| format!("Failed to receive stats: {}", e))?;
    
    let response = &buffer[..size];
    
    // Parse response
    if response.len() < 16 || response[0] != STAT_TYPE {
        return Err("Invalid stats response".to_string());
    }
    
    let mut offset = 16; // Skip header
    let mut properties = HashMap::new();
    
    // Read key-value pairs
    loop {
        let key = match extract_null_terminated_string(response, offset) {
            Ok(k) if !k.is_empty() => k,
            _ => break,
        };
        offset += key.len() + 1;
        
        let value = extract_null_terminated_string(response, offset)?;
        offset += value.len() + 1;
        
        properties.insert(key, value);
    }
    
    // Skip padding (10 bytes)
    offset += 10;
    
    // Read player list
    let mut players = Vec::new();
    while offset < response.len() {
        let player = match extract_null_terminated_string(response, offset) {
            Ok(p) if !p.is_empty() => p,
            _ => break,
        };
        offset += player.len() + 1;
        players.push(player);
    }
    
    Ok(QueryInfo {
        motd: properties.get("hostname").unwrap_or(&String::new()).clone(),
        game_type: properties.get("gametype").unwrap_or(&String::new()).clone(),
        map: properties.get("map").unwrap_or(&String::new()).clone(),
        players,
        version: properties.get("version").unwrap_or(&String::new()).clone(),
        plugins: properties.get("plugins").cloned(),
    })
}

fn extract_null_terminated_string(data: &[u8], start: usize) -> Result<String, String> {
    if start >= data.len() {
        return Err("Start position out of bounds".to_string());
    }
    
    let end = data[start..]
        .iter()
        .position(|&b| b == 0)
        .ok_or("String not null-terminated".to_string())?
        + start;
    
    String::from_utf8(data[start..end].to_vec())
        .map_err(|_| "Invalid UTF-8".to_string())
}