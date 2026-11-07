//! Cliente da IA remota: polimento LLM via Ollama e sincronização do perfil
//! de personalização (`fala-profile`) — um só cérebro para todos os dispositivos.
//! A rede nunca pode bloquear o ditado: erros sobem como Result e o chamador
//! (lib.rs) degrada para o texto sem polir.

use crate::settings::AiSettings;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Prompt de sistema padrão do polimento, usado até o primeiro sync do perfil.
pub fn default_system_prompt() -> &'static str {
    "Você recebe transcrições de ditado em português do Brasil. Corrija apenas pontuação, acentuação, capitalização e numeração, para o texto ficar como escrita natural. Não altere o sentido, não resuma, não acrescente nada e não responda ao conteúdo. Preserve nomes próprios e termos técnicos. Devolva SOMENTE o texto corrigido, sem comentários e sem aspas."
}

/// Perfil de personalização compartilhado no servidor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub vocab_prompt: Option<String>,
    #[serde(default)]
    pub corrections: Option<String>,
}

fn base_url(endpoint: &str, what: &str) -> Result<String, String> {
    let e = endpoint.trim().trim_end_matches('/');
    (!e.is_empty())
        .then(|| e.to_string())
        .ok_or_else(|| format!("{what} não configurado"))
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

fn bearer(token: &str) -> Option<(&'static str, String)> {
    let t = token.trim();
    (!t.is_empty()).then(|| ("Authorization", format!("Bearer {t}")))
}

/// Erros HTTP legíveis: status + corpo curto; transporte → mensagem direta.
fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let body: String = body.chars().take(300).collect();
            format!("HTTP {code}: {body}")
        }
        e => e.to_string(),
    }
}

/// Polimento da revisão final: POST /api/chat no Ollama (stream desligado —
/// queremos a frase inteira de uma vez, não streaming).
pub fn polish(cfg: &AiSettings, system_prompt: &str, text: &str) -> Result<String, String> {
    let url = format!("{}/api/chat", base_url(&cfg.endpoint, "endpoint do Ollama")?);
    let timeout = Duration::from_secs(u64::from(cfg.timeout_secs.max(2)));
    let resp = agent(timeout)
        .post(&url)
        .send_json(serde_json::json!({
            "model": cfg.model,
            "stream": false,
            "options": { "temperature": 0.2 },
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": text },
            ],
        }))
        .map_err(http_err)?;
    let body: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    body.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(str::trim)
        .map(str::to_string)
        .ok_or_else(|| format!("resposta inesperada do Ollama: {body}"))
}

/// Testa a conexão: GET /api/tags → nomes dos modelos instalados.
pub fn check(cfg: &AiSettings) -> Result<Vec<String>, String> {
    let url = format!("{}/api/tags", base_url(&cfg.endpoint, "endpoint do Ollama")?);
    let resp = agent(Duration::from_secs(8))
        .get(&url)
        .call()
        .map_err(http_err)?;
    let body: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    Ok(body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

fn profile_url(cfg: &AiSettings) -> Result<String, String> {
    Ok(format!(
        "{}/profile",
        base_url(&cfg.profile_url, "URL de personalização")?
    ))
}

/// Puxa o perfil compartilhado do servidor.
pub fn get_profile(cfg: &AiSettings) -> Result<Profile, String> {
    let url = profile_url(cfg)?;
    let mut req = agent(Duration::from_secs(8)).get(&url);
    if let Some((k, v)) = bearer(&cfg.token) {
        req = req.set(k, &v);
    }
    let resp = req.call().map_err(http_err)?;
    resp.into_json().map_err(|e| e.to_string())
}

/// Envia o perfil local ao servidor (o app vence ao salvar).
pub fn put_profile(cfg: &AiSettings, profile: &Profile) -> Result<(), String> {
    let url = profile_url(cfg)?;
    let mut req = agent(Duration::from_secs(8)).put(&url);
    if let Some((k, v)) = bearer(&cfg.token) {
        req = req.set(k, &v);
    }
    let body = serde_json::to_value(profile).map_err(|e| e.to_string())?;
    req.send_json(body).map_err(http_err)?;
    Ok(())
}
