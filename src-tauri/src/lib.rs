// Fala — ditador local push-to-talk (macOS + Windows).
// Segure o atalho global → grava; solte → transcreve com whisper.cpp local → cola no app focado.

mod audio;
mod inject;
mod settings;
mod stt;

use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use device_query::DeviceQuery;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Status {
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
}

struct AppState {
    settings: Mutex<settings::Settings>,
    recording: Mutex<Option<Recording>>,
    status: Mutex<Status>,
    transcriber: stt::Transcriber,
}

impl AppState {
    fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }
    fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }
}

// ---------------------------------------------------------------- ciclo push-to-talk

fn start_recording(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.status() != Status::Idle {
        return; // já gravando/transcrevendo
    }
    let settings = state.settings.lock().unwrap().clone();
    if let Err(e) = begin_recording(app, &settings) {
        eprintln!("[fala] falha ao iniciar gravação: {e}");
        state.set_status(Status::Idle);
        update_tray(app, Status::Idle, Some(&format!("Fala — erro: {e}")));
        hud_state(app, "error", Some(&e));
    }
}

fn begin_recording(app: &AppHandle, settings: &settings::Settings) -> Result<(), String> {
    let state = app.state::<AppState>();

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
    *state.recording.lock().unwrap() = Some(Recording {
        recorder,
        started: Instant::now(),
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

        // timeout sem fala: mic mudo/indisponível não pode deixar "ouvindo infinito"
        if !vad.speech_seen.load(Ordering::Relaxed) && elapsed > NO_SPEECH_TIMEOUT {
            eprintln!("[fala] {NO_SPEECH_TIMEOUT_SECS}s sem detectar fala; descartando gravação");
            discard_recording(&handle);
            hud_state(&handle, "error", Some("Nenhum áudio detectado — fale mais perto do microfone ou verifique o dispositivo nas configurações"));
            schedule_hud_hide(&handle, 3000);
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
            hud_state(app, "error", Some(&e));
            schedule_hud_hide(app, 2200);
            return;
        }
    };
    eprintln!(
        "[fala] gravação: {:.1}s, {} amostras @16k ({:.2}s de áudio)",
        elapsed.as_secs_f32(),
        samples.len(),
        samples.len() as f32 / 16_000.0
    );
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
    eprintln!("[fala] níveis (2s cada):{}", mapa.join(" |"));

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
        let started = Instant::now();
        let result = app
            .state::<AppState>()
            .transcriber
            .transcribe(&samples, &model_path, &language, vocab_prompt.as_deref())
            .map(|mut text| {
                // correções automáticas do usuário: "errado = correto", uma por linha
                if let Some(corr) = &corrections {
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
            });

        let payload = match result {
            Ok(text) if text.is_empty() => {
                eprintln!("[fala] transcrição vazia (áudio sem fala?)");
                serde_json::json!({ "ok": false, "error": "Nada reconhecido — fale mais perto do microfone." })
            }
            Ok(text) => {
                eprintln!(
                    "[fala] transcrito em {:.2}s: {text}",
                    started.elapsed().as_secs_f32()
                );
                if let Err(e) = inject::paste_text(&app, &text, &paste_mode) {
                    eprintln!("[fala] falha ao inserir texto: {e}");
                    serde_json::json!({ "ok": false, "error": format!("Falha ao colar: {e}"), "text": text })
                } else {
                    serde_json::json!({ "ok": true, "text": text })
                }
            }
            Err(e) => {
                eprintln!("[fala] erro na transcrição: {e}");
                serde_json::json!({ "ok": false, "error": e })
            }
        };

        let state = app.state::<AppState>();
        state.set_status(Status::Idle);
        update_tray(&app, Status::Idle, None);
        let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            hud_state(&app, "done", None);
            schedule_hud_hide(&app, 1400);
        } else {
            let msg = payload
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("erro desconhecido");
            hud_state(&app, "error", Some(msg));
            schedule_hud_hide(&app, 2400);
        }
        let _ = app.emit("transcription", payload);
    });
}

// ---------------------------------------------------------------- tray

const TRAY_ID: &str = "fala-tray";

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

const HUD_W: f64 = 300.0;
const HUD_H: f64 = 112.0;

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

fn hud_state(app: &AppHandle, state: &str, message: Option<&str>) {
    let _ = app.emit(
        "hud-state",
        serde_json::json!({ "state": state, "message": message }),
    );
}

fn schedule_hud_hide(app: &AppHandle, after_ms: u64) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(after_ms));
        hide_hud(&handle);
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
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<SettingsInfo, String> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    Ok(SettingsInfo {
        config_path: settings::config_path(&app)?.to_string_lossy().into_owned(),
        status: state.status().as_str(),
        accessibility: inject::accessibility_status(),
        settings,
    })
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
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
    Ok(())
}

#[tauri::command]
fn list_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn accessibility_status() -> bool {
    inject::accessibility_status()
}

#[tauri::command]
fn open_accessibility_prefs() {
    inject::open_accessibility_prefs();
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
        ])
        .setup(|app| {
            let settings = settings::load(app.handle());
            app.manage(AppState {
                recording: Mutex::new(None),
                status: Mutex::new(Status::Idle),
                transcriber: stt::Transcriber::new(),
                settings: Mutex::new(settings.clone()),
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
                        "quit" => app.exit(0),
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
            // reativar o app (clique no Dock) abre as configurações
            if let RunEvent::Reopen { .. } = event {
                show_settings_window(app);
            }
        });
}
