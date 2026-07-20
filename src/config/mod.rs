//! Configuration module.
//!
//! Loads and saves connection settings from a local TOML file.
//! Placeholder — full implementation coming in Phase 1.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application configuration stored in a local TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub qdrant_url: String,
    pub embedding_url: String,
    pub default_collection: Option<String>,
    pub embedding_model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            qdrant_url: "http://localhost:6333".to_string(),
            embedding_url: "http://localhost:8080/v1/embeddings".to_string(),
            default_collection: None,
            embedding_model: "BGE-M3".to_string(),
        }
    }
}

impl Config {
    /// Path to the config file.
    pub fn path() -> PathBuf {
        // TODO: use platform-appropriate config dir (e.g., dirs crate)
        PathBuf::from("echo.toml")
    }

    /// Load config from disk, returning defaults if the file doesn't exist.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Save config to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(Self::path(), contents)?;
        Ok(())
    }
}
