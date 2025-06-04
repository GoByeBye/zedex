use actix_web::{HttpResponse, Responder};
use log::debug;
use serde::{Serialize, Deserialize};
use std::time::{SystemTime, UNIX_EPOCH};
use once_cell::sync::OnceCell;

/// Health check response structure
#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    /// Status of the service
    status: String,
    /// Reason for the status
    reason: String,
    /// Version of the service
    version: String,
    /// Timestamp of the response
    timestamp: u64,
    /// Uptime in seconds
    uptime: u64,
    /// Number of extensions loaded
    extensions_loaded: u64,
}


/// Server uptime tracking
static SERVER_START_TIME: OnceCell<u64> = OnceCell::new();

/// Initialize the health check module
pub fn init() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    SERVER_START_TIME.set(now).ok();
}

/// Get the server start time
fn get_start_time() -> u64 {
    *SERVER_START_TIME.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })
}

/// Health check handler that returns service status in JSON format
pub async fn health_check() -> impl Responder {
    debug!("Health check requested");
    
    // Get current time
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    // Calculate uptime
    let uptime = now - get_start_time();
    
    // Create health response
    let mut health = HealthResponse {
        status: "OK".to_string(),
        reason: "Service is running".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: now,
        extensions_loaded: get_extensions_loaded_count(),
        uptime,
    };

    // Check for loaded extensions
    if health.extensions_loaded == 0 {
        health.status = "ERROR".to_string();
        health.reason = "No extensions found".to_string();
    }
    
    // Return JSON response
    if health.status == "OK" {
        HttpResponse::Ok().json(health)
    } else {
        HttpResponse::InternalServerError().json(health)
    }

}

pub fn get_extensions_loaded_count() -> u64 {
    let dir = std::env::var("ZED_EXTENSIONS_LOCAL_DIR").unwrap_or_else(|_| ".zedex-cache".to_string());
    match std::fs::read_dir(&dir) {
        Ok(entries) => entries.count() as u64,
        Err(_) => 0, // If the directory doesn't exist or can't be read, return 0
    }
}