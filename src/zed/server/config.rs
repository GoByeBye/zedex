use std::path::PathBuf;

#[derive(Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub extensions_dir: PathBuf,
    pub releases_dir: Option<PathBuf>,
    pub proxy_mode: bool,
    pub domain: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let root_dir = PathBuf::from(".zedex-cache");
        Self {
            port: 2654,
            host: "127.0.0.1".to_string(),
            extensions_dir: root_dir.clone(),
            releases_dir: Some(root_dir.join("releases")),
            proxy_mode: false,
            domain: None,
        }
    }
}
