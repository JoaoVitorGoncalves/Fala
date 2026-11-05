use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Atalho global push-to-talk (formato tauri-plugin-global-shortcut, ex: "alt+space")
    pub hotkey: String,
    /// Idioma de transcrição: "pt", "en", "auto", ...
    pub language: String,
    /// Nome do dispositivo de entrada; None = entrada padrão do sistema
    pub device: Option<String>,
    /// Caminho do modelo ggml do whisper
    pub model_path: Option<String>,
    /// Iniciar junto com o login
    pub autostart: bool,
    /// Limite de segurança da gravação (segundos)
    pub max_seconds: u32,
    /// "clipboard" (colar via Cmd/Ctrl+V) ou "type" (digitar caractere a caractere)
    pub paste_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "alt+space".into(),
            language: "pt".into(),
            device: None,
            model_path: default_model_path(),
            autostart: false,
            max_seconds: 120,
            paste_mode: "clipboard".into(),
        }
    }
}

/// Se o modelo do POC de voz já existir na máquina, usa como padrão (zero download).
fn default_model_path() -> Option<String> {
    let candidates = [
        dirs_home()?.join(".zcode/voice-poc/models/ggml-large-v3-turbo-q5_0.bin"),
        dirs_home()?.join(".fala/models/ggml-large-v3-turbo-q5_0.bin"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).or_else(|| {
        // Windows sem HOME: usa USERPROFILE
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    })
}

pub fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app_config_dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &tauri::AppHandle) -> Settings {
    let path = match config_path(app) {
        Ok(p) => p,
        Err(_) => return Settings::default(),
    };
    match fs::read_to_string(&path) {
        Ok(txt) => serde_json::from_str(&txt).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(app: &tauri::AppHandle, s: &Settings) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let txt = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    fs::write(&path, txt).map_err(|e| format!("write: {e}"))
}
