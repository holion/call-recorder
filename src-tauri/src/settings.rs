use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionProvider {
    Local,
    Openai,
}

impl Default for TranscriptionProvider {
    fn default() -> Self {
        Self::Openai
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub transcription_provider: TranscriptionProvider,
}

impl Settings {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = data_dir.join("settings.json");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}
