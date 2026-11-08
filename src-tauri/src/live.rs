// Transcrição em tempo real por COMMITS: cada pausa da fala fecha um trecho,
// o trecho é decodificado UMA vez e digitado — é texto final, nunca mais
// revisitado. Com Metal ativo (macOS) os trechos usam o MODELO GRANDE (a
// latência fica abaixo do small-na-CPU da v1); sem GPU, o lib.rs aponta o
// worker para o modelo rápido dedicado. Fala contínua força o fechamento num
// teto (corte no ponto mais silencioso recente) para manter cada decode curto.
// Ao soltar o atalho, só a cauda aberta é revisada (lib.rs). Nada do que já
// foi digitado é apagado nem re-transcrito. O Silero VAD (opcional) refina as
// bordas de cada trecho e descarta os sem fala nenhuma.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::audio::{self, RecorderHandle};
use crate::inject;
use crate::{apply_corrections, diag, AppState, Status};

const TICK: Duration = Duration::from_millis(120);
/// Pausa que fecha um trecho e o confirma (bem abaixo do auto-stop `silence_ms`)
const PAUSE: Duration = Duration::from_millis(400);
/// Trecho mínimo para valer um decode; abaixo disso espera a próxima pausa
const MIN_CHUNK_SAMPLES: usize = 9_600; // 0.6s @16 kHz
/// Fala contínua: teto do trecho aberto (mantém cada decode curto e o texto fluindo)
const MAX_CHUNK_SAMPLES: usize = 6 * 16_000;
/// Silêncio à direita incluído no trecho fechado (borda limpa para o whisper)
const TAIL_PAD_SAMPLES: usize = 4_000; // 0.25s @16 kHz
/// RMS mínimo para considerar voz no tick (mesmo limiar do trim_silence)
const VOICE_RMS: f32 = 0.006;
/// Prompt de continuidade: cauda do texto já confirmado
const PROMPT_CHARS: usize = 224;

pub struct LiveResult {
    /// Texto confirmado e digitado (diagnóstico + semântica da sessão).
    pub committed_text: String,
    /// Áudio 16 kHz do trecho que ficou aberto — revisado pelo modelo grande
    /// no encerramento, no lib.rs.
    pub tail: Vec<f32>,
}

pub fn spawn(
    app: AppHandle,
    audio_handle: RecorderHandle,
    model_path: String,
    language: String,
    vocab_prompt: Option<String>,
    corrections: Option<String>,
    generation: u64,
) -> std::thread::JoinHandle<LiveResult> {
    std::thread::spawn(move || {
        run(
            app,
            audio_handle,
            &model_path,
            &language,
            vocab_prompt,
            corrections,
            generation,
        )
    })
}

fn run(
    app: AppHandle,
    audio_handle: RecorderHandle,
    model_path: &str,
    language: &str,
    vocab_prompt: Option<String>,
    corrections: Option<String>,
    generation: u64,
) -> LiveResult {
    let rate = audio_handle.sample_rate();

    // Silero VAD (opcional): refina bordas do trecho e pula decodes sem fala.
    let mut vad = crate::settings::vad_model_path().and_then(|path| {
        let mut params = WhisperVadContextParams::new();
        params.set_use_gpu(false); // VAD é minúsculo — CPU sobra
        match WhisperVadContext::new(&path, params) {
            Ok(c) => Some(c),
            Err(e) => {
                diag(&format!("live: VAD indisponível ({e}); usando bordas por RMS"));
                None
            }
        }
    });
    diag(&format!(
        "live: ativo (modelo {model_path}, VAD: {})",
        vad.is_some()
    ));

    let mut native_cursor: usize = 0;
    let mut samples16k: Vec<f32> = Vec::new();
    let mut committed_until: usize = 0;
    let mut committed_text = String::new();
    // última voz vista: índice da amostra e o instante (para medir a pausa)
    let mut last_voice_idx: usize = 0;
    let mut last_voice_at: Option<Instant> = None;

    loop {
        std::thread::sleep(TICK);
        let state = app.state::<AppState>();
        if state.status() != Status::Recording
            || state.shutting_down.load(Ordering::Acquire)
            // sessão nova começou (auto-cura reinicia a gravação): esta sessão
            // está órfã — sai sem digitar nada
            || state.hud_generation.load(Ordering::Relaxed) != generation
        {
            break;
        }
        let new_native = audio_handle.snapshot_since(native_cursor);
        if !new_native.is_empty() {
            native_cursor += new_native.len();
            let chunk = audio::resample_to_16k(&new_native, rate);
            let rms = audio::rms_of(&chunk);
            samples16k.extend_from_slice(&chunk);
            if rms > VOICE_RMS {
                last_voice_idx = samples16k.len();
                last_voice_at = Some(Instant::now());
            }
        }

        let uncommitted = samples16k.len() - committed_until;
        let has_voice = last_voice_idx > committed_until;
        let paused = last_voice_at
            .map(|t| t.elapsed() >= PAUSE)
            .unwrap_or(false);

        // fecha o trecho: pausa após fala, ou teto de fala contínua
        let boundary = if has_voice && paused && uncommitted >= MIN_CHUNK_SAMPLES {
            Some((last_voice_idx + TAIL_PAD_SAMPLES).min(samples16k.len()))
        } else if uncommitted >= MAX_CHUNK_SAMPLES {
            Some(quietest_boundary(&samples16k, committed_until))
        } else {
            None
        };
        let Some(boundary) = boundary else {
            continue;
        };

        let chunk = &samples16k[committed_until..boundary];

        // VAD refina o trecho: corta silêncio das bordas e pula decodes sem
        // fala nenhuma (menos palavras fantasmas e decode desperdiçado).
        let speech = match vad.as_mut() {
            Some(v) => match vad_span(v, chunk) {
                VadSpan::NoSpeech => {
                    diag("live: trecho sem fala (VAD); pulando decode");
                    committed_until = boundary;
                    continue;
                }
                VadSpan::Span(range) => &chunk[range],
                VadSpan::Full => chunk,
            },
            None => chunk,
        };

        let started = Instant::now();
        let prompt = chunk_prompt(&vocab_prompt, &committed_text);
        let text = app
            .state::<AppState>()
            .transcriber
            .decode_segment(speech, model_path, language, prompt.as_deref());
        let text = match text {
            Ok(t) => t,
            Err(e) => {
                diag(&format!("live: decode do trecho falhou ({e}); cauda vai para a revisão final"));
                break;
            }
        };
        let text = apply_corrections(normalize_stream(&text), &corrections);
        diag(&format!(
            "live: trecho confirmado em {:.2}s ({:.1}s de áudio): {text}",
            started.elapsed().as_secs_f32(),
            chunk.len() as f32 / 16_000.0
        ));

        // Re-checa após o decode: a gravação pode ter sido encerrada/descartada
        // enquanto ele rodava — texto de sessão morta não pode ser digitado.
        {
            let state = app.state::<AppState>();
            if state.status() != Status::Recording
                || state.shutting_down.load(Ordering::Acquire)
                || state.hud_generation.load(Ordering::Relaxed) != generation
            {
                break;
            }
        }

        if text.is_empty() {
            // trecho sem fala reconhecida (ruído/respiração): só avança o cursor
            committed_until = boundary;
            continue;
        }
        let sep = if committed_text.is_empty() { "" } else { " " };
        let to_type = format!("{sep}{text}");
        if let Err(e) = inject::type_text_on_main(&app, &to_type) {
            diag(&format!(
                "live: digitação falhou ({e}); restante vai para a revisão final"
            ));
            // não avança committed_until: o trecho não digitado entra na cauda
            // e é entregue pela revisão final (nada é perdido nem duplicado)
            break;
        }
        committed_text.push_str(&to_type);
        committed_until = boundary;
        // versão limpa: o trecho cru vai para o worker de polimento async —
        // nunca bloqueia a digitação do próximo trecho
        crate::enqueue_polish(&app, text.clone());
    }

    LiveResult {
        committed_text,
        tail: samples16k[committed_until.min(samples16k.len())..].to_vec(),
    }
}

/// Resultado do VAD sobre um trecho.
enum VadSpan {
    /// Nenhuma fala (silêncio/ruído): pula o decode
    NoSpeech,
    /// Intervalo de fala (amostras relativas ao trecho)
    Span(std::ops::Range<usize>),
    /// VAD falhou: decodifica o trecho inteiro
    Full,
}

/// Ajusta o trecho à fala real detectada pelo Silero (fala já vem com pad de
/// 60ms aplicado pelo próprio VAD).
fn vad_span(vad: &mut WhisperVadContext, chunk: &[f32]) -> VadSpan {
    let mut params = WhisperVadParams::new();
    params.set_threshold(0.5);
    params.set_min_speech_duration(250); // ms
    params.set_min_silence_duration(100); // ms
    params.set_max_speech_duration(30.0); // s (teto já dado pelo MAX_CHUNK)
    params.set_speech_pad(60); // ms
    let Ok(segments) = vad.segments_from_samples(params, chunk) else {
        return VadSpan::Full;
    };
    let n = segments.num_segments();
    if n <= 0 {
        return VadSpan::NoSpeech;
    }
    let Some(first) = segments.get_segment_start_timestamp(0) else {
        return VadSpan::Full;
    };
    let Some(last) = segments.get_segment_end_timestamp(n - 1) else {
        return VadSpan::Full;
    };
    let start = (first * 16_000.0) as usize;
    let end = (last * 16_000.0) as usize;
    let start = start.min(chunk.len());
    let end = end.clamp(start, chunk.len());
    if end <= start {
        VadSpan::Full
    } else {
        VadSpan::Span(start..end)
    }
}

/// Ponto de corte para fala contínua: centro do intervalo de 100ms mais
/// silencioso dos últimos 2s — a aposta mais segura de um intervalo entre
/// palavras (o trecho fica final, então cortar no meio de uma palavra custa caro).
fn quietest_boundary(samples: &[f32], committed_until: usize) -> usize {
    const BIN: usize = 1_600; // 100ms @16 kHz
    let window_start = samples.len().saturating_sub(32_000).max(committed_until);
    let mut best = window_start;
    let mut best_rms = f32::MAX;
    let mut i = window_start;
    while i < samples.len() {
        let end = (i + BIN).min(samples.len());
        let rms = audio::rms_of(&samples[i..end]);
        if rms < best_rms {
            best_rms = rms;
            best = i;
        }
        i += BIN;
    }
    (best + BIN / 2).min(samples.len())
}

/// Prompt do trecho: vocabulário do usuário + cauda do texto já confirmado
/// (mantém nomes e estilo consistentes entre trechos).
fn chunk_prompt(vocab_prompt: &Option<String>, committed_text: &str) -> Option<String> {
    let mut prompt = String::new();
    if let Some(v) = vocab_prompt {
        let v = v.trim();
        if !v.is_empty() {
            prompt.push_str(v);
        }
    }
    let tail = tail_chars(committed_text, PROMPT_CHARS);
    if let Some(t) = tail {
        if !prompt.is_empty() {
            prompt.push(' ');
        }
        prompt.push_str(&t);
    }
    if prompt.is_empty() {
        None
    } else {
        Some(prompt)
    }
}

/// Últimos `n` chars de `s`.
fn tail_chars(s: &str, n: usize) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let chars: Vec<char> = t.chars().collect();
    let start = chars.len().saturating_sub(n);
    Some(chars[start..].iter().collect())
}

/// Normaliza a saída de um trecho: espaços colapsados e reticências limpas.
fn normalize_stream(raw: &str) -> String {
    let t = raw.trim().trim_start_matches(['…', '.']).trim_end_matches('…');
    let t = t.trim().trim_end_matches("...");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}
