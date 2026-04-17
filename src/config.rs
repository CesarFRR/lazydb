use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct UiConfig {
    pub rows_per_page: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { rows_per_page: 10 }
    }
}

pub fn load_ui_config() -> UiConfig {
    let path = config_file_path();

    let Ok(content) = fs::read_to_string(path) else {
        return UiConfig::default();
    };

    let Ok(parsed) = content.parse::<toml::Value>() else {
        return UiConfig::default();
    };

    let rows_per_page = parsed
        .get("ui")
        .and_then(|ui| ui.get("rows_per_page"))
        .and_then(toml::Value::as_integer)
        .and_then(|value| u32::try_from(value).ok())
        .map_or(10, |value| value.clamp(1, 500));

    UiConfig { rows_per_page }
}

fn config_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("lazydb").join("config.toml")
}
