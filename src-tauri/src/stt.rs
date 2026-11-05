use std::path::Path;
use std::sync::Mutex;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

/// Transcritor local: carrega o modelo uma vez e reusa o contexto.
pub struct Transcriber {
    ctx: Mutex<Option<WhisperContext>>,
    loaded_from: Mutex<Option<String>>,
}

impl Transcriber {
    pub fn new() -> Self {
        Self {
            ctx: Mutex::new(None),
            loaded_from: Mutex::new(None),
        }
    }

    /// Garante o contexto carregado com o modelo indicado; recarrega se o caminho mudou.
    fn ensure_model(&self, model_path: &str) -> Result<(), String> {
        let mut guard = self.ctx.lock().map_err(|e| e.to_string())?;
        let mut loaded = self.loaded_from.lock().map_err(|e| e.to_string())?;
        if guard.is_some() && loaded.as_deref() == Some(model_path) {
            return Ok(());
        }
        if !Path::new(model_path).exists() {
            return Err(format!("Modelo não encontrado: {model_path}"));
        }
        eprintln!("[fala:stt] carregando modelo: {model_path}");
        let ctx = WhisperContext::new_with_params(
            model_path,
            WhisperContextParameters {
                // GPU (Metal no macOS) quando disponível
                use_gpu: true,
                ..Default::default()
            },
        )
        .map_err(|e| format!("carregar modelo: {e}"))?;
        *guard = Some(ctx);
        *loaded = Some(model_path.to_string());
        eprintln!("[fala:stt] modelo carregado");
        Ok(())
    }

    /// Transcreve amostras f32 16 kHz mono. `language = "auto"` usa detecção automática.
    pub fn transcribe(&self, samples: &[f32], model_path: &str, language: &str) -> Result<String, String> {
        if samples.is_empty() {
            return Err("Áudio vazio — nada capturado do microfone.".into());
        }
        self.ensure_model(model_path)?;
        let mut guard = self.ctx.lock().map_err(|e| e.to_string())?;
        let ctx = guard.as_mut().ok_or("contexto do whisper ausente")?;
        let mut state = ctx.create_state().map_err(|e| format!("create_state: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        if language != "auto" {
            params.set_language(Some(language));
        }

        state
            .full(params, samples)
            .map_err(|e| format!("whisper full: {e}"))?;
        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str() {
                    text.push_str(s);
                }
            }
        }
        Ok(clean_text(&text))
    }
}

fn clean_text(raw: &str) -> String {
    let t = raw.trim();
    // descarta saídas que são só marcadores de não-fala
    if t.is_empty()
        || t == "."
        || t == "[BLANK_AUDIO]"
        || t == "[MUSIC]"
        || t.eq_ignore_ascii_case("(silence)")
    {
        String::new()
    } else {
        t.to_string()
    }
}
