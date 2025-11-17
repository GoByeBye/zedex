use std::sync::Arc;

use super::config::ServerConfig;

#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<ServerConfig>,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    pub fn config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }
}
