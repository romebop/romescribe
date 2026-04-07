use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub selected_model: String,
    pub use_gpu: bool,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub copy_to_clipboard: bool,
    #[serde(default)]
    pub audio_device: String,
}

fn default_hotkey() -> String {
    "Cmd+Shift+R".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            selected_model: "small.en".to_string(),
            use_gpu: true,
            hotkey: default_hotkey(),
            copy_to_clipboard: false,
            audio_device: String::new(),
        }
    }
}

fn settings_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.romescribe.dev");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app directory: {}", e))?;
    Ok(app_dir.join("settings.json"))
}

pub fn load_settings() -> Settings {
    match settings_path() {
        Ok(path) => {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                    Err(_) => Settings::default(),
                }
            } else {
                Settings::default()
            }
        }
        Err(_) => Settings::default(),
    }
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path()?;
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write settings: {}", e))?;
    Ok(())
}
