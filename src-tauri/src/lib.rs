// Fala — ditador local push-to-talk (macOS + Windows).
// Segure o atalho global → grava; solte → transcreve com whisper.cpp local → cola no app focado.

mod audio;
mod inject;
mod settings;
mod stt;

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
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
    let model_ok = match &settings.model_path {
        Some(p) if std::path::Path::new(p).exists() => true,
        Some(p) => {
            eprintln!("[fala] modelo não encontrado: {p}");
            update_tray(
                app,
                Status::Idle,
                Some("Fala — modelo não encontrado; abra as configurações"),
            );
            show_settings_window(app);
            return;
        }
        None => {
            eprintln!("[fala] modelo não configurado");
            update_tray(
                app,
                Status::Idle,
                Some("Fala — configure o modelo nas configurações"),
            );
            show_settings_window(app);
            return;
        }
    };
    let _ = model_ok;

    match audio::Recorder::start(settings.device.as_deref()) {
        Ok(recorder) => {
            *state.recording.lock().unwrap() = Some(Recording {
                recorder,
                started: Instant::now(),
            });
            state.set_status(Status::Recording);
            update_tray(app, Status::Recording, None);

            // guarda de segurança: se o keyup se perder, força parada no limite
            let handle = app.clone();
            let max = settings.max_seconds.max(3);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(u64::from(max)));
                let state = handle.state::<AppState>();
                if state.status() == Status::Recording {
                    eprintln!("[fala] limite de {max}s atingido; finalizando");
                    stop_recording_and_transcribe(&handle);
                }
            });
        }
        Err(e) => {
            eprintln!("[fala] falha ao iniciar gravação: {e}");
            update_tray(app, Status::Idle, Some(&format!("Fala — erro: {e}")));
        }
    }
}

fn stop_recording_and_transcribe(app: &AppHandle) {
    let state = app.state::<AppState>();
    let recording = state.recording.lock().unwrap().take();
    let Some(recording) = recording else {
        return; // não estava gravando
    };
    state.set_status(Status::Transcribing);
    update_tray(app, Status::Transcribing, None);

    let elapsed = recording.started.elapsed();
    let samples = match recording.recorder.stop() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[fala] falha ao encerrar gravação: {e}");
            state.set_status(Status::Idle);
            update_tray(app, Status::Idle, Some(&format!("Fala — erro: {e}")));
            return;
        }
    };
    eprintln!(
        "[fala] gravação: {:.1}s, {} amostras @16k",
        elapsed.as_secs_f32(),
        samples.len()
    );

    let settings = state.settings.lock().unwrap().clone();
    let model_path = settings.model_path.unwrap_or_default();
    let language = settings.language.clone();
    let paste_mode = settings.paste_mode.clone();

    let app = app.clone(); // handle próprio para a thread de transcrição
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = app
            .state::<AppState>()
            .transcriber
            .transcribe(&samples, &model_path, &language);

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
        let _ = app.emit("transcription", payload);
    });
}

// ---------------------------------------------------------------- tray

const TRAY_ID: &str = "fala-tray";

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
    match event.state() {
        ShortcutState::Pressed => start_recording(app),
        ShortcutState::Released => stop_recording_and_transcribe(app),
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
