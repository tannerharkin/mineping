use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 25565;
pub const TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub online: bool,
    pub host: String,
    pub port: u16,
    pub resolved_from: String,
    pub version: Option<VersionInfo>,
    pub players: Option<PlayersInfo>,
    pub description: Option<String>,
    pub favicon: Option<String>,
    pub latency_ms: Option<u64>,
    pub query_enabled: bool,
    pub protocol_used: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub name: String,
    pub protocol: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayersInfo {
    pub online: i32,
    pub max: i32,
    pub sample: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryInfo {
    pub motd: String,
    pub game_type: String,
    pub map: String,
    pub players: Vec<String>,
    pub version: String,
    pub plugins: Option<String>,
}

// VarInt encoding/decoding for Minecraft protocol
pub fn write_varint(value: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut val = value as u32;
    
    loop {
        if (val & !0x7F) == 0 {
            bytes.push(val as u8);
            break;
        }
        bytes.push((val & 0x7F | 0x80) as u8);
        val >>= 7;
    }
    
    bytes
}

pub fn read_varint(data: &[u8], offset: &mut usize) -> Result<i32, String> {
    let mut value = 0;
    let mut position = 0;
    
    loop {
        if *offset >= data.len() {
            return Err("VarInt exceeds data length".to_string());
        }
        
        let byte = data[*offset];
        *offset += 1;
        
        value |= ((byte & 0x7F) as i32) << position;
        
        if (byte & 0x80) == 0 {
            break;
        }
        
        position += 7;
        
        if position >= 32 {
            return Err("VarInt is too big".to_string());
        }
    }
    
    Ok(value)
}

pub fn write_string(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut result = write_varint(bytes.len() as i32);
    result.extend_from_slice(bytes);
    result
}

pub fn read_string(data: &[u8], offset: &mut usize) -> Result<String, String> {
    let length = read_varint(data, offset)? as usize;
    
    if *offset + length > data.len() {
        return Err("String length exceeds data".to_string());
    }
    
    let string_bytes = &data[*offset..*offset + length];
    *offset += length;
    
    String::from_utf8(string_bytes.to_vec())
        .map_err(|_| "Invalid UTF-8".to_string())
}