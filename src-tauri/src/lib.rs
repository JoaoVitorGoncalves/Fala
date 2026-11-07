// Fala — ditador local push-to-talk (macOS + Windows).
// Segure o atalho global → grava; solte → transcreve com whisper.cpp local → cola no app focado.

mod audio;
mod inject;
mod live;
mod llm;
mod settings;
mod stt;

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use device_query::DeviceQuery;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Status {
    Idle,
    Recording,
    Transcribing,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Recording => "recording",
            Status::Transcribing => "transcribing",
        }
    }
}

struct Recording {
    recorder: audio::Recorder,
    started: Instant,
    hud_generation: u64,
    /// worker de transcrição em tempo real (None se desativado/sem modelo)
    live: Option<std::thread::JoinHandle<live::LiveResult>>,
}

struct AppState {
    settings: Mutex<settings::Settings>,
    recording: Mutex<Option<Recording>>,
    status: Mutex<Status>,
    transcriber: stt::Transcriber,
    /// tentativas consecutivas sem fala (auto-cura pós-concessão de permissão)
    no_speech_retries: std::sync::atomic::AtomicU32,
    /// Identifica a operação de gravação/transcrição que possui o HUD atual.
    hud_generation: AtomicU64,
    shutting_down: AtomicBool,
    active_transcriptions: AtomicU32,
    shutdown_waiter_started: AtomicBool,
    /// download do modelo rápido em andamento (evita downloads concorrentes)
    live_download: AtomicBool,
}

impl AppState {
    fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }
    pub(crate) fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }
}

/// Correções automáticas do usuário: "errado = correto", uma por linha.
/// Aplicadas a cada trecho confirmado e à revisão final da cauda.
pub(crate) fn apply_corrections(mut text: String, corrections: &Option<String>) -> String {
    if let Some(corr) = corrections {
        for line in corr.lines() {
            let mut parts = line.splitn(2, '=');
            let from = parts.next().unwrap_or("").trim();
            let to = parts.next().unwrap_or("").trim();
            if !from.is_empty() {
                text = text.replace(from, to);
            }
        }
    }
    text
}

/// Polimento IA da cauda revisada (trechos ao vivo não passam pela IA — já
/// foram digitados como texto final). Falha é silenciosa: diag + texto sem
/// polir; a rede nunca pode bloquear o ditado.
fn polish_tail(app: &AppHandle, generation: u64, text: String) -> String {
    let Some(ai) = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .ai
        .clone()
        .filter(|ai| ai.enabled)
    else {
        return text;
    };
    if text.trim().is_empty() {
        return text;
    }
    // HUD só se esta sessão ainda é a dona dele (nova gravação não pode ser sobrescrita)
    let state = app.state::<AppState>();
    if state.hud_generation.load(Ordering::Relaxed) == generation
        && state.status() == Status::Transcribing
    {
        hud_state(app, "transcribing", Some("Polindo com IA…"));
    }
    let sys = ai
        .system_prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| llm::default_system_prompt().into());
    let started = Instant::now();
    match llm::polish(&ai, &sys, &text) {
        Ok(p) if !p.trim().is_empty() => {
            diag(&format!(
                "polimento IA em {:.2}s: {p}",
                started.elapsed().as_secs_f32()
            ));
            p
        }
        Ok(_) => {
            diag("polimento IA retornou vazio; mantendo o texto bruto");
            text
        }
        Err(e) => {
            diag(&format!("polimento IA falhou ({e}); mantendo o texto bruto"));
            text
        }
    }
}

// ---------------------------------------------------------------- ciclo push-to-talk

fn start_recording(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.shutting_down.load(Ordering::Acquire) {
        return;
    }
    if state.status() != Status::Idle {
        return; // já gravando/transcrevendo
    }
    let settings = state.settings.lock().unwrap().clone();
    if let Err(e) = begin_recording(app, &settings) {
        eprintln!("[fala] falha ao iniciar gravação: {e}");
        state.set_status(Status::Idle);
        update_tray(app, Status::Idle, Some(&format!("Fala — erro: {e}")));
        hud_state(app, "error", Some(&e));
        show_hud(app);
        schedule_hud_hide(app, 2200, state.hud_generation.load(Ordering::Relaxed));
    }
}

fn begin_recording(app: &AppHandle, settings: &settings::Settings) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.shutting_down.load(Ordering::Acquire) {
        return Err("o app está encerrando".into());
    }
    // Invalidate any older transcription/hide timer before starting a new session.
    let generation = state.hud_generation.fetch_add(1, Ordering::Relaxed) + 1;

    match &settings.model_path {
        Some(p) if std::path::Path::new(p).exists() => {}
        Some(p) => {
            eprintln!("[fala] modelo não encontrado: {p}");
            show_settings_window(app);
            return Err("modelo não encontrado; abra as configurações".into());
        }
        None => {
            eprintln!("[fala] modelo não configurado");
            show_settings_window(app);
            return Err("configure o modelo nas configurações".into());
        }
    }

    let vad = Arc::new(audio::VadState::new());
    let recorder = audio::Recorder::start(settings.device.as_deref(), vad.clone())?;
    // transcrição em tempo real: worker próprio confirmando trechos a cada
    // pausa. No macOS (Metal) os trechos usam o modelo grande — latência de
    // small-na-CPU com precisão do large; em CPU pura (Windows), o modelo
    // rápido dedicado. Sem modelo/sem live: gravação segue como antes.
    let live_model = if cfg!(target_os = "macos") {
        settings.model_path.clone()
    } else {
        settings::live_model_path_or_default(settings)
    };
    let live = match live_model {
        Some(path) if settings.live_transcribe => Some(live::spawn(
            app.clone(),
            recorder.handle(),
            path,
            settings.language.clone(),
            settings.vocab_prompt.clone(),
            settings.corrections.clone(),
            generation,
        )),
        _ => None,
    };
    *state.recording.lock().unwrap() = Some(Recording {
        recorder,
        started: Instant::now(),
        hud_generation: generation,
        live,
    });
    state.set_status(Status::Recording);
    update_tray(app, Status::Recording, None);
    hud_state(app, "recording", None);
    show_hud(app);

    // thread de supervisão: emite níveis p/ o HUD, auto-stop por silêncio,
    // timeout sem fala e tecla Esc para finalizar
    let handle = app.clone();
    let vad = vad;
    // device_query PANICA sem Acessibilidade (macos/mod.rs:122) — só cria o
    // monitor de teclado se o status disser que temos a permissão, e cada uso
    // passa por catch_unwind (a permissão pode sumir entre builds ad-hoc)
    let mut keyboard: Option<device_query::DeviceState> = None;
    if inject::accessibility_status() {
        keyboard = std::panic::catch_unwind(device_query::DeviceState::new).ok();
    }
    let mode = settings.mode.clone();
    let silence = Duration::from_millis(u64::from(settings.silence_ms.clamp(400, 10_000)));
    let max = Duration::from_secs(u64::from(settings.max_seconds.max(3)));
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(45));
        let state = handle.state::<AppState>();
        if state.shutting_down.load(Ordering::Acquire) {
            break;
        }
        if state.status() != Status::Recording {
            break; // parada externa (keyup / toggle)
        }
        let rms = vad.current_rms();
        let _ = handle.emit("hud-level", rms);
        let elapsed = vad.started.elapsed();

        // Esc em qualquer lugar finaliza a gravação (stop + transcrição);
        // re-tenta criar o monitor se a Acessibilidade foi concedida agora
        if keyboard.is_none() && inject::accessibility_status() {
            keyboard = std::panic::catch_unwind(device_query::DeviceState::new).ok();
        }
        if let Some(kb) = &keyboard {
            let pressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                kb.get_keys()
            }));
            if let Ok(keys) = pressed {
                if keys.contains(&device_query::Keycode::Escape) {
                    eprintln!("[fala] Esc pressionado; finalizando gravação");
                    stop_recording_and_transcribe(&handle);
                    break;
                }
            }
        }

        if elapsed > max {
            eprintln!("[fala] limite de {}s atingido; finalizando", max.as_secs());
            stop_recording_and_transcribe(&handle);
            break;
        }

        // timeout sem fala: mic mudo/indisponível não pode deixar "ouvindo infinito".
        // 1ª ocorrência: reinicia a gravação (o stream pode ter nascido antes da
        // concessão do TCC ficar ativa); 2ª consecutiva: descarta com aviso.
        if !vad.speech_seen.load(Ordering::Relaxed) && elapsed > NO_SPEECH_TIMEOUT {
            let retries = state
                .no_speech_retries
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if retries < 1 {
                eprintln!(
                    "[fala] {}s sem fala; reiniciando a gravação (auto-cura, tentativa {})",
                    NO_SPEECH_TIMEOUT_SECS,
                    retries + 1
                );
                diag(&format!(
                    "sem fala após {:.1}s; reiniciando gravação (tentativa {})",
                    elapsed.as_secs_f32(),
                    retries + 1
                ));
                if let Some(recording) = state.recording.lock().unwrap().take() {
                    let _ = recording.recorder.stop();
                }
                state.set_status(Status::Idle);
                let settings = state.settings.lock().unwrap().clone();
                if let Err(e) = begin_recording(&handle, &settings) {
                    hud_state(&handle, "error", Some(&e));
                    schedule_hud_hide(
                        &handle,
                        2400,
                        state.hud_generation.load(Ordering::Relaxed),
                    );
                }
                break;
            }
            eprintln!("[fala] 2ª tentativa sem fala; descartando");
            diag("sem fala na 2ª tentativa; descartando gravação");
            discard_recording(&handle);
            hud_state(
                &handle,
                "error",
                Some(
                    "Nenhum áudio detectado — fale mais perto do microfone ou troque o dispositivo nas configurações",
                ),
            );
            schedule_hud_hide(
                &handle,
                3000,
                state.hud_generation.load(Ordering::Relaxed),
            );
            break;
        }

        if mode == "toggle" {
            if let Some(sil) = vad.silence_since() {
                if sil > silence {
                    eprintln!(
                        "[fala] silêncio de {:.1}s após fala; encerrando automaticamente",
                        sil.as_secs_f32()
                    );
                    stop_recording_and_transcribe(&handle);
                    break;
                }
            }
        }
    });
    Ok(())
}

/// Encerra a gravação descartando o áudio (timeout sem fala).
fn discard_recording(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(recording) = state.recording.lock().unwrap().take() {
        let _ = recording.recorder.stop(); // amostras vão fora
    }
    state.set_status(Status::Idle);
    update_tray(app, Status::Idle, None);
}

fn stop_recording_and_transcribe(app: &AppHandle) {
    let state = app.state::<AppState>();
    let recording = state.recording.lock().unwrap().take();
    let Some(recording) = recording else {
        return; // não estava gravando
    };
    let generation = recording.hud_generation;
    let live_handle = recording.live;
    state.active_transcriptions.fetch_add(1, Ordering::AcqRel);
    state.set_status(Status::Transcribing);
    update_tray(app, Status::Transcribing, None);
    hud_state(app, "transcribing", None);

    let elapsed = recording.started.elapsed();
    let samples = match recording.recorder.stop() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[fala] falha ao encerrar gravação: {e}");
            state.set_status(Status::Idle);
            update_tray(app, Status::Idle, Some(&format!("Fala — erro: {e}")));
            state.active_transcriptions.fetch_sub(1, Ordering::AcqRel);
            hud_state(app, "error", Some(&e));
            schedule_hud_hide(app, 2200, generation);
            return;
        }
    };
    diag(&format!(
        "gravação: {:.1}s, {} amostras @16k ({:.2}s de áudio)",
        elapsed.as_secs_f32(),
        samples.len(),
        samples.len() as f32 / 16_000.0
    ));
    // mapa de níveis (RMS por 2s) — mostra se o mic "morre" no meio do stream
    let níveis = audio::level_map(&samples, 16_000, 2);
    let mapa: Vec<String> = níveis
        .iter()
        .map(|rms| {
            if *rms < 0.0005 {
                "  .  ".into() // silêncio digital
            } else if *rms < 0.01 {
                " baixo".into()
            } else {
                " FALA ".into()
            }
        })
        .collect();
    diag(&format!("níveis (2s cada):{}", mapa.join(" |")));

    let settings = state.settings.lock().unwrap().clone();
    let model_path = settings.model_path.unwrap_or_default();
    let language = settings.language.clone();
    let paste_mode = settings.paste_mode.clone();
    let vocab_prompt = settings
        .vocab_prompt
        .clone()
        .filter(|p| !p.trim().is_empty());
    let corrections = settings.corrections.clone();

    let app = app.clone(); // handle próprio para a thread de transcrição
    std::thread::spawn(move || {
        // Aguarda o worker ao vivo: trechos fechados já foram digitados como
        // texto FINAL; sobrou apenas a cauda aberta para revisão.
        let live = match live_handle {
            Some(h) => match h.join() {
                Ok(r) => r,
                Err(_) => {
                    diag("live: worker encerrou com panic; seguindo sem commits");
                    live::LiveResult {
                        committed_text: String::new(),
                        tail: Vec::new(),
                    }
                }
            },
            None => live::LiveResult {
                committed_text: String::new(),
                tail: Vec::new(),
            },
        };

        let started = Instant::now();
        // Revisão rápida: só a cauda aberta passa pelo modelo grande. Sem live
        // (ou tudo já confirmado), a cauda é o ditado inteiro — comportamento
        // clássico. Correções do usuário e o polimento IA se aplicam ao resultado.
        let result = if live.tail.is_empty() {
            Ok(live.committed_text.clone())
        } else {
            app.state::<AppState>()
                .transcriber
                .transcribe(&live.tail, &model_path, &language, vocab_prompt.as_deref())
                .map(|text| apply_corrections(text, &corrections))
                .map(|text| polish_tail(&app, generation, text))
        };

        let payload = match result {
            Ok(text) if text.is_empty() => {
                if !live.committed_text.is_empty() {
                    // a cauda era silêncio (ou nada reconhecido), mas os trechos
                    // confirmados já estão no input — ditado bem-sucedido
                    diag("cauda vazia/sem fala; mantendo trechos confirmados do live");
                    app.state::<AppState>()
                        .no_speech_retries
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    serde_json::json!({ "ok": true, "text": live.committed_text })
                } else {
                    diag("transcrição vazia (áudio sem fala?)");
                    serde_json::json!({ "ok": false, "error": "Nada reconhecido — fale mais perto do microfone." })
                }
            }
            Ok(text) => {
                diag(&format!(
                    "revisão da cauda em {:.2}s: {text}",
                    started.elapsed().as_secs_f32()
                ));
                app.state::<AppState>()
                    .no_speech_retries
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                if live.tail.is_empty() {
                    // tudo já foi digitado pelos commits ao vivo
                    serde_json::json!({ "ok": true, "text": text })
                } else {
                    // insere a cauda revisada — nada existente é apagado
                    let sep = if live.committed_text.is_empty() { "" } else { " " };
                    match inject::paste_text(&app, &format!("{sep}{text}"), &paste_mode) {
                        Ok(()) => serde_json::json!({ "ok": true, "text": text }),
                        Err(e) => {
                            diag(&format!("falha ao inserir a cauda: {e}"));
                            serde_json::json!({ "ok": false, "error": format!("Falha ao inserir: {e}"), "text": text })
                        }
                    }
                }
            }
            Err(e) => {
                if !live.committed_text.is_empty() {
                    // a revisão da cauda falhou, mas os trechos confirmados já
                    // estão no input — nada se perde
                    diag(&format!("erro na revisão da cauda ({e}); mantendo trechos confirmados"));
                    serde_json::json!({ "ok": true, "text": live.committed_text })
                } else {
                    diag(&format!("erro na transcrição: {e}"));
                    serde_json::json!({ "ok": false, "error": e })
                }
            }
        };

        let state = app.state::<AppState>();
        state.active_transcriptions.fetch_sub(1, Ordering::AcqRel);
        // If another recording has already started, this worker is stale and
        // must not overwrite its HUD or schedule a hide for the new session.
        if state.hud_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        state.set_status(Status::Idle);
        update_tray(&app, Status::Idle, None);
        let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            hud_state(&app, "done", None);
            schedule_hud_hide(&app, 1400, generation);
        } else {
            let msg = payload
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("erro desconhecido");
            hud_state(&app, "error", Some(msg));
            schedule_hud_hide(&app, 2400, generation);
        }
        let _ = app.emit("transcription", payload);
    });
}

// ---------------------------------------------------------------- tray

const TRAY_ID: &str = "fala-tray";

/// Loga em stderr e em ~/.fala/fala.log (diagnóstico sem terminal).
pub(crate) fn diag(msg: &str) {
    eprintln!("[fala] {msg}");
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join(".fala/fala.log");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{ts}] {msg}");
        }
    }
}

/// Se nenhuma fala for detectada neste intervalo, a gravação é descartada
/// (microfone mudo/indisponível não pode deixar o app "ouvindo infinito").
const NO_SPEECH_TIMEOUT_SECS: u64 = 7;
const NO_SPEECH_TIMEOUT: Duration = Duration::from_secs(NO_SPEECH_TIMEOUT_SECS);

fn update_tray(app: &AppHandle, status: Status, tooltip_override: Option<&str>) {
    // NSStatusItem (tray) exige a main thread no macOS; esta função também é
    // chamada de threads de gravação/transcrição, então marshal para a principal.
    let tray_app = app.clone();
    let tooltip: String = tooltip_override
        .map(str::to_string)
        .unwrap_or_else(|| match status {
            Status::Idle => "Fala — pronto (segure o atalho para ditar)".into(),
            Status::Recording => "Fala — gravando…".into(),
            Status::Transcribing => "Fala — transcrevendo…".into(),
        });
    let _ = app.run_on_main_thread(move || {
        let Some(tray) = tray_app.tray_by_id(TRAY_ID) else {
            return;
        };
        let _ = tray.set_title(match status {
            Status::Idle => None,
            Status::Recording => Some("● rec"),
            Status::Transcribing => Some("… txt"),
        });
        let _ = tray.set_tooltip(Some(&tooltip));
    });
}

fn show_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// ---------------------------------------------------------------- HUD flutuante

const HUD_W: f64 = 320.0;
const HUD_H: f64 = 84.0; // cápsula fixa do HUD público (radius 42 no conf)

/// Mostra o HUD no centro-inferior da tela, SEM roubar o foco do app alvo
/// (focusable=false → canBecomeKeyWindow=false → o paste continua indo certo).
fn show_hud(app: &AppHandle) {
    let Some(win) = app.get_webview_window("hud") else {
        return;
    };
    let _ = win.set_focusable(false);
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let sf = monitor.scale_factor();
        let mpos = monitor.position();
        let msize = monitor.size();
        let w = (HUD_W * sf).round() as i32;
        let h = (HUD_H * sf).round() as i32;
        let x = mpos.x + (msize.width as i32 - w) / 2;
        let y = mpos.y + msize.height as i32 - h - (150.0 * sf).round() as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = win.show();
}

fn hide_hud(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("hud") {
        let _ = win.hide();
    }
}

fn request_shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.shutting_down.swap(true, Ordering::AcqRel) {
        return;
    }
    state.hud_generation.fetch_add(1, Ordering::AcqRel);
    if let Some(recording) = state.recording.lock().unwrap().take() {
        let _ = recording.recorder.stop();
    }
    state.set_status(Status::Idle);
    // O teardown de residency do Metal do whisper.cpp dá assert se o contexto
    // cai durante o app.exit — vaza os contextos deliberadamente (o processo
    // está saindo; o SO devolve a memória).
    state.transcriber.leak();
    let _ = app.global_shortcut().unregister_all();
    hide_hud(app);

    // Whisper/Metal must not be dropped while a transcription worker is still
    // using the persistent context. Defer the actual process exit until the
    // worker has released its WhisperState and decremented this counter.
    if state.active_transcriptions.load(Ordering::Acquire) == 0 {
        app.exit(0);
        return;
    }
    if state
        .shutdown_waiter_started
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || loop {
        let state = handle.state::<AppState>();
        if state.active_transcriptions.load(Ordering::Acquire) == 0 {
            handle.exit(0);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    });
}

fn hud_state(app: &AppHandle, state: &str, message: Option<&str>) {
    let _ = app.emit(
        "hud-state",
        serde_json::json!({ "state": state, "message": message }),
    );
}

fn schedule_hud_hide(app: &AppHandle, after_ms: u64, generation: u64) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(after_ms));
        let state = handle.state::<AppState>();
        if state.hud_generation.load(Ordering::Relaxed) == generation
            && state.status() == Status::Idle
        {
            hide_hud(&handle);
        }
    });
}

// ---------------------------------------------------------------- atalho global

fn apply_shortcut(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| format!("unregister_all: {e}"))?;
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|e| format!("atalho inválido \"{hotkey}\": {e:?}"))?;
    gs.register(shortcut)
        .map_err(|e| format!("register \"{hotkey}\": {e}"))?;
    Ok(())
}

fn on_shortcut(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    let mode = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .mode
        .clone();
    eprintln!("[fala] evento do atalho: {:?} (modo {mode})", event.state());
    match event.state() {
        ShortcutState::Pressed => match mode.as_str() {
            "toggle" => {
                let status = app.state::<AppState>().status();
                match status {
                    Status::Idle => start_recording(app),
                    Status::Recording => stop_recording_and_transcribe(app),
                    Status::Transcribing => {}
                }
            }
            _ => start_recording(app), // push: segurar
        },
        ShortcutState::Released => {
            if mode == "push" {
                stop_recording_and_transcribe(app);
            }
        }
    }
}

// ---------------------------------------------------------------- comandos (UI)

#[derive(Serialize)]
struct SettingsInfo {
    settings: settings::Settings,
    config_path: String,
    status: &'static str,
    accessibility: bool,
    /// modelo rápido p/ transcrição ao vivo: existe no disco (configurado ou default)?
    live_model_available: bool,
    /// caminho resolvido do modelo rápido ("" se ausente)
    live_model_path: String,
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<SettingsInfo, String> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let live_path = settings::live_model_path_or_default(&settings);
    Ok(SettingsInfo {
        config_path: settings::config_path(&app)?.to_string_lossy().into_owned(),
        status: state.status().as_str(),
        accessibility: inject::accessibility_status(),
        live_model_available: live_path.is_some(),
        live_model_path: live_path.unwrap_or_default(),
        settings,
    })
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
    let previous = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .clone();
    settings::save(&app, &settings)?;
    {
        let state = app.state::<AppState>();
        *state.settings.lock().unwrap() = settings.clone();
    }
    apply_shortcut(&app, &settings.hotkey)?;
    let autolaunch = app.autolaunch();
    if settings.autostart {
        autolaunch
            .enable()
            .map_err(|e| format!("autostart enable: {e}"))?;
    } else {
        autolaunch
            .disable()
            .map_err(|e| format!("autostart disable: {e}"))?;
    }
    // Personalização mudou e a IA está configurada → envia ao servidor para
    // valer nos outros dispositivos (fire-and-forget; falha só diag/evento).
    if let Some(ai) = &settings.ai {
        if ai.enabled && !ai.profile_url.trim().is_empty() && profile_changed(&previous, &settings)
        {
            push_profile(&app);
        }
    }
    Ok(())
}

#[tauri::command]
fn list_devices() -> Vec<String> {
    audio::list_input_devices()
}

/// Baixa o modelo rápido das parciais ao vivo (~181 MB) para ~/.fala/models via
/// curl. Emite `model-download-progress {received}` durante e `{done, error?}`
/// no fim. O .part quebra/mantém atomicidade: só o rename publica o modelo.
#[tauri::command]
fn download_live_model(app: AppHandle) -> Result<(), String> {
    if app
        .state::<AppState>()
        .live_download
        .swap(true, Ordering::AcqRel)
    {
        return Err("download já em andamento".into());
    }
    let Some(home) = std::env::var_os("HOME") else {
        app.state::<AppState>().live_download.store(false, Ordering::Release);
        return Err("variável HOME não definida".into());
    };
    let dir = std::path::Path::new(&home).join(".fala/models");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        app.state::<AppState>().live_download.store(false, Ordering::Release);
        return Err(format!("create_dir_all: {e}"));
    }
    let final_path = dir.join(settings::LIVE_MODEL_FILE);
    if final_path.exists() {
        app.state::<AppState>().live_download.store(false, Ordering::Release);
        return Ok(());
    }
    let part_path = dir.join(format!("{}.part", settings::LIVE_MODEL_FILE));

    // progresso: tamanho do .part enquanto o curl roda (metadata falhou = fim)
    let prog_app = app.clone();
    let prog_part = part_path.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        let Ok(meta) = std::fs::metadata(&prog_part) else {
            break;
        };
        let _ = prog_app.emit(
            "model-download-progress",
            serde_json::json!({ "received": meta.len() }),
        );
    });

    let dl_app = app.clone();
    std::thread::spawn(move || {
        let url = format!("{}/{}", settings::LIVE_MODEL_BASE, settings::LIVE_MODEL_FILE);
        let outcome = (|| -> Result<(), String> {
            let status = std::process::Command::new("curl")
                .args(["-fL", "--retry", "3", "-o"])
                .arg(&part_path)
                .arg(&url)
                .status()
                .map_err(|e| format!("curl: {e}"))?;
            if !status.success() {
                return Err(format!("curl saiu com {status}"));
            }
            std::fs::rename(&part_path, &final_path).map_err(|e| format!("rename: {e}"))?;
            Ok(())
        })();
        dl_app
            .state::<AppState>()
            .live_download
            .store(false, Ordering::Release);
        let payload = match outcome {
            Ok(()) => {
                diag("modelo rápido baixado");
                serde_json::json!({ "done": true })
            }
            Err(e) => {
                diag(&format!("download do modelo rápido falhou: {e}"));
                let _ = std::fs::remove_file(&part_path); // .part quebrado não fica
                serde_json::json!({ "done": false, "error": e })
            }
        };
        let _ = dl_app.emit("model-download-progress", payload);
    });
    Ok(())
}

#[tauri::command]
fn accessibility_status() -> bool {
    inject::accessibility_status()
}

#[tauri::command]
fn open_accessibility_prefs() {
    inject::open_accessibility_prefs();
}

// ---------------------------------------------------------------- IA (servidor)

/// Puxa o perfil de personalização do servidor e aplica localmente.
/// O servidor vence no boot e no botão "Sincronizar"; o app vence ao salvar.
fn sync_profile_blocking(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ai = state
        .settings
        .lock()
        .unwrap()
        .ai
        .clone()
        .filter(|ai| !ai.profile_url.trim().is_empty())
        .ok_or("IA do servidor não configurada")?;
    let profile = llm::get_profile(&ai)?;
    let mut s = state.settings.lock().unwrap().clone();
    if let Some(a) = &mut s.ai {
        a.system_prompt = profile.system_prompt.or_else(|| a.system_prompt.clone());
    }
    s.vocab_prompt = profile.vocab_prompt.or_else(|| s.vocab_prompt.clone());
    s.corrections = profile.corrections.or_else(|| s.corrections.clone());
    settings::save(app, &s)?;
    *state.settings.lock().unwrap() = s;
    Ok(())
}

/// Sincroniza o perfil de personalização com o servidor; emite
/// `profile-synced {ok, error?}` para a UI atualizar vocabulário/correções.
#[tauri::command]
fn sync_profile(app: AppHandle) -> Result<(), String> {
    let h = app.clone();
    std::thread::spawn(move || {
        let payload = match sync_profile_blocking(&h) {
            Ok(()) => {
                diag("perfil de personalização sincronizado do servidor");
                serde_json::json!({ "ok": true })
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        let _ = h.emit("profile-synced", payload);
    });
    Ok(())
}

/// Testa a conexão com o Ollama configurado; emite `ai-test-result {ok, models?, error?}`.
#[tauri::command]
fn test_ai(app: AppHandle) -> Result<(), String> {
    let ai = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .ai
        .clone()
        .ok_or("IA do servidor não configurada")?;
    let h = app.clone();
    std::thread::spawn(move || {
        let payload = match llm::check(&ai) {
            Ok(models) => serde_json::json!({ "ok": true, "models": models }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        let _ = h.emit("ai-test-result", payload);
    });
    Ok(())
}

/// Write-through da personalização: o app vence ao salvar. Roda em thread e
/// reporta pelo mesmo evento `profile-synced` (com `pushed`).
fn push_profile(app: &AppHandle) {
    let h = app.clone();
    std::thread::spawn(move || {
        let outcome = (|| -> Result<(), String> {
            let state = h.state::<AppState>();
            let ai = state
                .settings
                .lock()
                .unwrap()
                .ai
                .clone()
                .ok_or("IA não configurada")?;
            let s = state.settings.lock().unwrap().clone();
            llm::put_profile(
                &ai,
                &llm::Profile {
                    system_prompt: ai.system_prompt.clone(),
                    vocab_prompt: s.vocab_prompt.clone(),
                    corrections: s.corrections.clone(),
                },
            )
        })();
        match outcome {
            Ok(()) => {
                diag("perfil de personalização enviado ao servidor");
                let _ = h.emit(
                    "profile-synced",
                    serde_json::json!({ "ok": true, "pushed": true }),
                );
            }
            Err(e) => {
                diag(&format!("falha ao enviar perfil ao servidor: {e}"));
                let _ = h.emit(
                    "profile-synced",
                    serde_json::json!({ "ok": false, "pushed": true, "error": e }),
                );
            }
        }
    });
}

/// A personalização mudou entre dois snapshots de settings?
fn profile_changed(a: &settings::Settings, b: &settings::Settings) -> bool {
    let sys = |s: &settings::Settings| s.ai.as_ref().and_then(|ai| ai.system_prompt.clone());
    sys(a) != sys(b) || a.vocab_prompt != b.vocab_prompt || a.corrections != b.corrections
}

// ---------------------------------------------------------------- bootstrap

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // panics vão para stderr E para ~/.fala/panic.log (apps LaunchServices não
    // entregam stderr ao log unificado de forma confiável)
    std::panic::set_hook(Box::new(|info| {
        let msg = match info.location() {
            Some(loc) => format!(
                "[fala][PANIC] {info} ({}:{}:{})",
                loc.file(),
                loc.line(),
                loc.column()
            ),
            None => format!("[fala][PANIC] {info}"),
        };
        eprintln!("{msg}");
        if let Some(home) = std::env::var_os("HOME") {
            let path = std::path::Path::new(&home).join(".fala/panic.log");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                use std::io::Write;
                let _ = writeln!(f, "{}", msg);
            }
        }
    }));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(on_shortcut)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            list_devices,
            accessibility_status,
            open_accessibility_prefs,
            download_live_model,
            sync_profile,
            test_ai,
        ])
        .setup(|app| {
            let settings = settings::load(app.handle());
            app.manage(AppState {
                recording: Mutex::new(None),
                status: Mutex::new(Status::Idle),
                transcriber: stt::Transcriber::new(),
                settings: Mutex::new(settings.clone()),
                no_speech_retries: std::sync::atomic::AtomicU32::new(0),
                hud_generation: AtomicU64::new(0),
                shutting_down: AtomicBool::new(false),
                active_transcriptions: AtomicU32::new(0),
                shutdown_waiter_started: AtomicBool::new(false),
                live_download: AtomicBool::new(false),
            });

            let show = MenuItem::with_id(app, "settings", "Configurações…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().expect("ícone padrão").clone())
                .tooltip("Fala — pronto")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| {
                    eprintln!("[fala] menu da bandeja: {}", ev.id.as_ref());
                    match ev.id.as_ref() {
                        "settings" => show_settings_window(app),
                        "quit" => request_shutdown(app),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        eprintln!("[fala] clique esquerdo na bandeja → configurações");
                        show_settings_window(tray.app_handle());
                    }
                })
                .build(app)?;

            if let Err(e) = apply_shortcut(app.handle(), &settings.hotkey) {
                eprintln!("[fala] {e}");
                update_tray(
                    app.handle(),
                    Status::Idle,
                    Some("Fala — atalho inválido; abra as configurações"),
                );
            }

            if settings.autostart {
                let _ = app.autolaunch().enable();
            }

            // IA do servidor: puxa o perfil compartilhado (um só para todos os
            // dispositivos). Servidor vence no boot; falha só diag — app segue offline.
            if settings
                .ai
                .clone()
                .is_some_and(|ai| ai.enabled && ai.sync_profile && !ai.profile_url.trim().is_empty())
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    if let Err(e) = sync_profile_blocking(&h) {
                        diag(&format!("sync do perfil no boot falhou: {e}"));
                    } else {
                        diag("perfil de personalização sincronizado no boot");
                    }
                });
            }

            eprintln!(
                "[fala] pronto — atalho: {} | idioma: {} | modelo: {}",
                settings.hotkey,
                settings.language,
                settings.model_path.as_deref().unwrap_or("(não configurado)")
            );
            Ok(())
        })
        .on_window_event(|window, event| {
            // fechar = esconder; o app vive na bandeja
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("erro ao construir o aplicativo Fala")
        .run(|app, event| {
            match event {
                // Cmd+Q, Dock quit, or another native exit request follows
                // the same coordinated path as the tray's "Sair" action.
                RunEvent::ExitRequested { api, .. } => {
                    let state = app.state::<AppState>();
                    if !state.shutting_down.load(Ordering::Acquire) {
                        api.prevent_exit();
                        request_shutdown(app);
                    }
                }
                // Reativar o app (clique no Dock) abre as configurações.
                RunEvent::Reopen { .. } => show_settings_window(app),
                _ => {}
            }
        });
}
