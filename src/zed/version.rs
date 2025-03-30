use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// Represents a Zed release version
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Version {
    pub version: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

impl Version {
    /// Parses semantic version components into (major, minor, patch)
    pub fn parse_semver(&self) -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = self.version.split('.').collect();
        if parts.len() < 3 {
            return None;
        }
        
        let major = parts[0].parse::<u32>().ok()?;
        let minor = parts[1].parse::<u32>().ok()?;
        let patch = parts[2].parse::<u32>().ok()?;
        
        Some((major, minor, patch))
    }
    
    /// Compare version semantically
    pub fn compare(&self, other: &Version) -> Ordering {
        match (self.parse_semver(), other.parse_semver()) {
            (Some((self_major, self_minor, self_patch)), 
             Some((other_major, other_minor, other_patch))) => {
                match self_major.cmp(&other_major) {
                    Ordering::Equal => {
                        match self_minor.cmp(&other_minor) {
                            Ordering::Equal => self_patch.cmp(&other_patch),
                            ordering => ordering,
                        }
                    },
                    ordering => ordering,
                }
            },
            _ => self.version.cmp(&other.version),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.version)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(std::cmp::Ord::cmp(self, other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(other)
    }
} 