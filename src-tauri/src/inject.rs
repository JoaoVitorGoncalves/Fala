use std::time::Duration;
use tauri::AppHandle;

/// Insere texto no app em foco. Estratégia padrão: clipboard → colar → restaurar.
/// Fallback: digitação sintética caractere a caractere (mais lenta, mais compatível).
///
/// IMPORTANTE (macOS): as APIs de simulação/teclado (enigo → HIToolbox/TIS) exigem a
/// main thread — por isso o enigo é executado via `run_on_main_thread`. Os períodos
/// de espera do clipboard ficam na thread chamadora para não travar a UI.

#[cfg(target_os = "macos")]
mod accessibility {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrusted() == 1 }
    }
}

#[cfg(target_os = "macos")]
pub fn accessibility_status() -> bool {
    accessibility::is_trusted()
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_status() -> bool {
    true // Windows não exige permissão equivalente para SendInput
}

#[cfg(target_os = "macos")]
pub fn open_accessibility_prefs() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn open_accessibility_prefs() {}

/// Executa `f` na main thread e aguarda o resultado (enigo é rápido; o bloqueio
/// aqui é na thread chamadora, que já é de fundo).
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("run_on_main_thread: {e}"))?;
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("aguardando main thread: {e}"))
}

pub fn paste_text(app: &AppHandle, text: &str, mode: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    let text = text.to_string();
    if mode == "type" {
        return on_main(app, move || type_text(text)).and_then(|r| r);
    }
    paste_via_clipboard(app, &text).or_else(|e| {
        eprintln!("[fala:inject] colagem falhou ({e}); caindo para digitação");
        on_main(app, move || type_text(text)).and_then(|r| r)
    })
}

fn paste_via_clipboard(app: &AppHandle, text: &str) -> Result<(), String> {
    use arboard::Clipboard;

    let mut clip = Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    // salva o conteúdo textual atual para restaurar depois (não-textual se perde)
    let saved = clip.get_text().ok();

    clip.set_text(text.to_string())
        .map_err(|e| format!("set_text: {e}"))?;

    // pequena espera para o app alvo enxergar o clipboard antes do Cmd+V
    std::thread::sleep(Duration::from_millis(80));
    on_main(app, paste_key_combo).and_then(|r| r)?;

    // dá tempo do app alvo processar a colagem antes de restaurar
    std::thread::sleep(Duration::from_millis(300));
    if let Some(prev) = saved {
        let _ = clip.set_text(prev);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn paste_key_combo() -> Result<(), String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Meta, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Meta, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn paste_key_combo() -> Result<(), String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}

fn type_text(text: String) -> Result<(), String> {
    use enigo::{Enigo, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.text(&text).map_err(|e| format!("enigo.text: {e}"))?;
    Ok(())
}
