use crate::api::Station;
use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq, Serialize, Deserialize)]
#[version = 4]
pub struct Config {
    #[serde(default)]
    pub favorites: Vec<Station>,
    #[serde(default)]
    pub volume: u8, // 0-100
}

impl Default for Config {
    fn default() -> Self {
        Self {
            favorites: Vec::new(),
            volume: 50,
        }
    }
}
