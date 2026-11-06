use std::path::Path;
use std::sync::Mutex;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

/// Transcritor local: carrega o modelo uma vez e reusa o contexto.
pub struct Transcriber {
    ctx: Mutex<Option<WhisperContext>>,
    loaded_from: Mutex<Option<String>>,
    /// Estado persistente reusado pelos decodes ao vivo (criar state por decode
    /// custa caro). O passe final continua criando o seu por chamada.
    live_state: Mutex<Option<WhisperState>>,
}

impl Transcriber {
    pub fn new() -> Self {
        Self {
            ctx: Mutex::new(None),
            loaded_from: Mutex::new(None),
            live_state: Mutex::new(None),
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
                // Metal ativo (macOS): o teardown de residency do whisper.cpp
                // dava assert no app.exit — mitigado vazando os contextos no
                // shutdown (Transcriber::leak, chamado de request_shutdown).
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
    /// `initial_prompt` (opcional) guia o modelo com vocabulário/estilo do usuário.
    pub fn transcribe(
        &self,
        samples: &[f32],
        model_path: &str,
        language: &str,
        initial_prompt: Option<&str>,
    ) -> Result<String, String> {
        let trimmed = trim_silence(samples);
        if trimmed.is_empty() {
            return Err("Áudio sem fala — só silêncio foi capturado.".into());
        }
        // Defesa extra: amostras não-finitas fazem o whisper.cpp abortar (assert
        // no ggml). A captura já sanitiza, mas qualquer NaN residual vira silêncio.
        let needs_sanitize = trimmed.iter().any(|v| !v.is_finite());
        let sanitized: Vec<f32>;
        let trimmed: &[f32] = if needs_sanitize {
            sanitized = trimmed.iter().map(|&v| if v.is_finite() { v } else { 0.0 }).collect();
            &sanitized
        } else {
            trimmed
        };
        self.ensure_model(model_path)?;
        let mut guard = self.ctx.lock().map_err(|e| e.to_string())?;
        let ctx = guard.as_mut().ok_or("contexto do whisper ausente")?;
        let mut state = ctx.create_state().map_err(|e| format!("create_state: {e}"))?;

        let params = base_params(language, initial_prompt);
        state
            .full(params, trimmed)
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

    /// Decode de um trecho ao vivo com o estado persistente (o contexto e o
    /// state sobrevivem entre gravações — carregar modelo por gravação custaria
    /// segundos). Sem trim_silence/clean_text: o live normaliza por conta própria.
    pub fn decode_segment(
        &self,
        samples: &[f32],
        model_path: &str,
        language: &str,
        initial_prompt: Option<&str>,
    ) -> Result<String, String> {
        self.ensure_model(model_path)?;
        let mut guard = self.live_state.lock().map_err(|e| e.to_string())?;
        if guard.is_none() {
            let ctx_guard = self.ctx.lock().map_err(|e| e.to_string())?;
            let ctx = ctx_guard.as_ref().ok_or("contexto do whisper ausente")?;
            *guard = Some(ctx.create_state().map_err(|e| format!("create_state: {e}"))?);
        }
        let state = guard.as_mut().expect("live_state criado acima");
        let params = base_params(language, initial_prompt);
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
        Ok(text)
    }

    /// Vaza deliberadamente os contextos no shutdown: o teardown de residency
    /// do Metal do whisper.cpp dá assert se o WhisperContext cai durante o
    /// app.exit. O processo está saindo — o SO devolve a memória.
    pub fn leak(&self) {
        if let Ok(mut g) = self.live_state.lock() {
            if let Some(s) = g.take() {
                std::mem::forget(s);
            }
        }
        if let Ok(mut g) = self.ctx.lock() {
            if let Some(c) = g.take() {
                std::mem::forget(c);
            }
        }
    }
}

/// Params base compartilhados entre o passe final e os trechos ao vivo.
pub(crate) fn base_params<'a>(
    language: &'a str,
    initial_prompt: Option<&'a str>,
) -> FullParams<'a, 'a> {
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
    if let Some(p) = initial_prompt {
        if !p.trim().is_empty() {
            params.set_initial_prompt(p);
        }
    }
    params
}

/// Corta silêncio das pontas (50ms por chunk): elimina a matéria-prima das
/// alucinações de fim de clipe ("Obrigado.", "Até logo.", etc.).
fn trim_silence(samples: &[f32]) -> &[f32] {
    const CHUNK: usize = 800; // 50 ms @16 kHz
    const THRESH: f32 = 0.006;
    let mut first: Option<usize> = None;
    let mut last = 0usize;
    for (i, chunk) in samples.chunks(CHUNK).enumerate() {
        let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
        let rms = (sum_sq / chunk.len().max(1) as f32).sqrt();
        if rms > THRESH {
            if first.is_none() {
                first = Some(i);
            }
            last = i;
        }
    }
    let Some(f) = first else {
        return &[];
    };
    let start = f.saturating_sub(2) * CHUNK;
    let end = ((last + 3) * CHUNK).min(samples.len());
    &samples[start..end]
}

fn clean_text(raw: &str) -> String {
    // remove reticências penduradas nas pontas (artefato comum do whisper)
    let t = raw
        .trim()
        .trim_start_matches(['…', '.', ' '])
        .trim_end_matches(['…', ' '])
        .trim_end_matches("...")
        .trim();
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
