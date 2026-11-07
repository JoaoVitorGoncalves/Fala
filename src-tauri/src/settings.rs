use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Atalho global (formato tauri-plugin-global-shortcut, ex: "alt+space")
    pub hotkey: String,
    /// "toggle" = aperta uma vez, para sozinho no silêncio | "push" = segurar
    pub mode: String,
    /// (toggle) silêncio que encerra a gravação automaticamente, em ms
    pub silence_ms: u32,
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
    /// Prompt inicial do whisper: vocabulário, nomes e termos que guião o reconhecimento
    pub vocab_prompt: Option<String>,
    /// Correções pós-transcrição, uma por linha: "errado = correto"
    pub corrections: Option<String>,
    /// Transcrição em tempo real: palavras surgem no input enquanto fala
    pub live_transcribe: bool,
    /// Modelo rápido (ggml) para as parciais ao vivo; None = usa o default detectável
    pub live_model_path: Option<String>,
    /// IA remota + perfil de personalização compartilhado (None = desativada)
    pub ai: Option<AiSettings>,
}

/// IA de polimento servida no servidor (Ollama via Tailscale) + o perfil
/// compartilhado (vocabulário, correções, prompt) — um só para todos os
/// dispositivos. O servidor vence no boot/sync; o app vence ao salvar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Polimento LLM ativo na revisão final da cauda (trechos ao vivo não passam pela IA)
    pub enabled: bool,
    /// URL base do Ollama — ex. http://fala-servidor:11434 (localhost = Ollama local)
    pub endpoint: String,
    /// URL do serviço de personalização (fala-profile) — ex. http://fala-servidor:8338
    pub profile_url: String,
    /// Token Bearer do serviço de personalização
    pub token: String,
    /// Modelo no Ollama — ex. llama3.1:8b-instruct-q4_K_M
    pub model: String,
    /// Timeout do polimento em segundos (estourar = texto entra sem polir)
    pub timeout_secs: u32,
    /// Puxar o perfil do servidor quando o app inicia
    pub sync_profile: bool,
    /// Prompt de sistema do polimento (sincronizado com o servidor)
    pub system_prompt: Option<String>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            profile_url: String::new(),
            token: String::new(),
            model: "llama3.1:8b-instruct-q4_K_M".into(),
            timeout_secs: 20,
            sync_profile: true,
            system_prompt: Some(crate::llm::default_system_prompt().to_string()),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "alt+space".into(),
            mode: "toggle".into(),
            silence_ms: 1500,
            language: "pt".into(),
            device: None,
            model_path: default_model_path(),
            autostart: false,
            max_seconds: 120,
            paste_mode: "clipboard".into(),
            vocab_prompt: None,
            corrections: None,
            live_transcribe: true,
            live_model_path: None,
            ai: None,
        }
    }
}

/// Nome do modelo rápido usado nas parciais em tempo real (baixável pelo app).
/// small usa quantização q5_1 no repo whisper.cpp (q5_0 não existe → 404).
pub const LIVE_MODEL_FILE: &str = "ggml-small-q5_1.bin";
pub const LIVE_MODEL_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Modelo Silero VAD (opcional, ~2 MB): refina as bordas dos trechos ao vivo.
pub const VAD_MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";

/// Caminho do modelo VAD se presente no disco (o live funciona sem ele).
pub fn vad_model_path() -> Option<String> {
    let path = dirs_home()?.join(".fala/models").join(VAD_MODEL_FILE);
    path.exists()
        .then(|| path.to_string_lossy().into_owned())
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

/// Resolve o modelo rápido efetivo: o configurado, ou o default se existir no
/// disco (permite baixar pelo app sem editar settings).
pub fn live_model_path_or_default(settings: &Settings) -> Option<String> {
    if let Some(p) = &settings.live_model_path {
        return if Path::new(p).exists() {
            Some(p.clone())
        } else {
            None
        };
    }
    let candidates = [
        dirs_home()?.join(".fala/models").join(LIVE_MODEL_FILE),
        dirs_home()?.join(".zcode/voice-poc/models").join(LIVE_MODEL_FILE),
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
