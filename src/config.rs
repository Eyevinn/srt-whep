use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DiscoverConfig {
    pub enable_discoverer: bool,
    pub discoverer_timeout_sec: u64,
}

impl DiscoverConfig {
    pub fn load(path: &str) -> Result<Self, Box<dyn Error>> {
        let config = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&config)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_should_be_loaded() {
        let result: Result<DiscoverConfig, toml::de::Error> =
            toml::from_str(include_str!("discover.conf"));
        assert!(result.is_ok());
    }
}
