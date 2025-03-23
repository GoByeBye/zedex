use serde::{Deserialize, Serialize};

/// Represents a Zed extension with its metadata
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub schema_version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_api_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default)]
    pub download_count: i32,
    #[serde(default)]
    pub provides: Vec<String>,
}

impl Extension {
    /// Check if this extension provides a specific capability
    pub fn provides_capability(&self, capability: &str) -> bool {
        self.provides.iter().any(|p| p == capability)
    }
}

/// A collection of extensions
pub type Extensions = Vec<Extension>;

/// Wrapper structure for JSON API responses
#[derive(Debug, Serialize, Deserialize)]
pub struct WrappedExtensions {
    pub data: Extensions,
}

/// Functions for working with Extensions without implementing directly on Vec
pub mod extensions_utils {
    use super::Extensions;
    use log::debug;

    /// Filter a collection of extensions by various criteria
    /// 
    /// # Arguments
    /// * `extensions` - The collection of extensions to filter
    /// * `filter` - Optional text to search in name, id, and description
    /// * `max_schema_version` - Optional maximum schema version
    /// * `provides` - Optional capability that extensions must provide
    pub fn filter_extensions(
        extensions: &Extensions,
        filter: Option<&str>,
        max_schema_version: Option<i32>,
        provides: Option<&str>,
    ) -> Extensions {
        debug!("Filtering extensions with criteria: filter={:?}, max_schema_version={:?}, provides={:?}", 
            filter, max_schema_version, provides);
            
        let filtered: Extensions = extensions
            .iter()
            .filter(|ext| {
                // Filter by max schema version if provided
                if let Some(max_version) = max_schema_version {
                    if ext.schema_version > max_version {
                        return false;
                    }
                }
                
                // Filter by text search if provided
                if let Some(search_text) = filter {
                    if !search_text.is_empty() && 
                       !ext.name.to_lowercase().contains(&search_text.to_lowercase()) &&
                       !ext.id.to_lowercase().contains(&search_text.to_lowercase()) &&
                       !ext.description.to_lowercase().contains(&search_text.to_lowercase()) {
                        return false;
                    }
                }
                
                // Filter by provides capability if provided
                if let Some(capability) = provides {
                    if !capability.is_empty() && !ext.provides_capability(capability) {
                        return false;
                    }
                }
                
                true
            })
            .cloned()
            .collect();
        
        debug!("Filtered extensions from {} to {}", extensions.len(), filtered.len());
        filtered
    }
} 