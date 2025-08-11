use crate::minecraft::common::*;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Debug, Deserialize, Serialize)]
struct StatusResponse {
    version: StatusVersion,
    players: StatusPlayers,
    description: Description,
    #[serde(skip_serializing_if = "Option::is_none")]
    favicon: Option<String>,
    #[serde(rename = "enforcesSecureChat", skip_serializing_if = "Option::is_none")]
    enforces_secure_chat: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StatusVersion {
    name: String,
    protocol: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct StatusPlayers {
    max: i32,
    online: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample: Option<Vec<PlayerSample>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlayerSample {
    name: String,
    id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum Description {
    String(String),
    Object { text: String },
    Complex { text: String, extra: Vec<TextComponent> },
}

#[derive(Debug, Deserialize, Serialize)]
struct TextComponent {
    text: String,
}

pub async fn ping(host: &str, port: u16, hostname: &str) -> Result<ServerInfo, String> {
    let timeout_duration = Duration::from_millis(TIMEOUT_MS);
    
    // Connect with timeout
    let mut stream = timeout(timeout_duration, TcpStream::connect((host, port)))
        .await
        .map_err(|_| "Connection timeout".to_string())?
        .map_err(|e| format!("Connection failed: {}", e))?;
    
    // Build handshake packet
    let mut handshake = Vec::new();
    handshake.push(0x00); // Packet ID
    handshake.extend(write_varint(-1)); // Protocol version -1 for status
    handshake.extend(write_string(hostname)); // Server address
    handshake.push((port >> 8) as u8); // Port high byte
    handshake.push((port & 0xFF) as u8); // Port low byte
    handshake.extend(write_varint(1)); // Next state: status
    
    // Send handshake
    send_packet(&mut stream, &handshake).await?;
    
    // Send status request
    send_packet(&mut stream, &[0x00]).await?;
    
    // Read status response
    let start = Instant::now();
    let response_data = read_packet(&mut stream).await?;
    let latency = start.elapsed().as_millis() as u64;
    
    // Parse response
    let mut offset = 0;
    let packet_id = response_data[0];
    offset += 1;
    
    if packet_id != 0x00 {
        return Err(format!("Unexpected packet ID: 0x{:02x}", packet_id));
    }
    
    let json_str = read_string(&response_data, &mut offset)?;
    let status: StatusResponse = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse JSON: {}", e))?;
    
    // Convert to our format
    let description = match status.description {
        Description::String(s) => s,
        Description::Object { text } => text,
        Description::Complex { text, extra } => {
            let mut result = text;
            for component in extra {
                result.push_str(&component.text);
            }
            result
        }
    };
    
    let player_names = status.players.sample.as_ref().map(|sample| {
        sample.iter().map(|p| p.name.clone()).collect()
    });
    
    Ok(ServerInfo {
        online: true,
        host: host.to_string(),
        port,
        resolved_from: "direct".to_string(),
        version: Some(VersionInfo {
            name: status.version.name,
            protocol: status.version.protocol,
        }),
        players: Some(PlayersInfo {
            online: status.players.online,
            max: status.players.max,
            sample: player_names,
        }),
        description: Some(description),
        favicon: status.favicon,
        latency_ms: Some(latency),
        query_enabled: false,
        protocol_used: "modern_slp".to_string(),
    })
}

async fn send_packet(stream: &mut TcpStream, data: &[u8]) -> Result<(), String> {
    let length = write_varint(data.len() as i32);
    stream.write_all(&length).await.map_err(|e| e.to_string())?;
    stream.write_all(data).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_packet(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    // Read packet length
    let mut length_bytes = vec![0u8; 5]; // Max VarInt size
    let mut length_size = 0;
    
    for i in 0..5 {
        stream.read_exact(&mut length_bytes[i..i+1]).await
            .map_err(|e| format!("Failed to read length: {}", e))?;
        length_size += 1;
        
        if (length_bytes[i] & 0x80) == 0 {
            break;
        }
    }
    
    let mut offset = 0;
    let length = read_varint(&length_bytes[..length_size], &mut offset)? as usize;
    
    if length > 65535 {
        return Err(format!("Packet too large: {}", length));
    }
    
    // Read packet data
    let mut data = vec![0u8; length];
    stream.read_exact(&mut data).await
        .map_err(|e| format!("Failed to read packet: {}", e))?;
    
    Ok(data)
}