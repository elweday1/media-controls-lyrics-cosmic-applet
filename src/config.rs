use cosmic::cosmic_config::{
    self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry,
};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    pub caching_enabled: bool,
    pub navidrome_url: String,
    pub navidrome_user: String,
    pub navidrome_password: String,
    pub player_whitelist: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            caching_enabled: true,
            navidrome_url: String::new(),
            navidrome_user: String::new(),
            navidrome_password: String::new(),
            player_whitelist: String::new(),
        }
    }
}
