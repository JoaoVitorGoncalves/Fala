/// Insere texto no app em foco. Estratégia padrão: clipboard → colar → restaurar.
/// Fallback: digitação sintética caractere a caractere (mais lenta, mais compatível).

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

pub fn paste_text(text: &str, mode: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    if mode == "type" {
        return type_text(text);
    }
    paste_via_clipboard(text).or_else(|e| {
        eprintln!("[fala:inject] colagem falhou ({e}); caindo para digitação");
        type_text(text)
    })
}

fn paste_via_clipboard(text: &str) -> Result<(), String> {
    use arboard::Clipboard;

    let mut clip = Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    // salva o conteúdo textual atual para restaurar depois (conteúdo não-textual se perde)
    let saved = clip.get_text().ok();

    clip.set_text(text.to_string())
        .map_err(|e| format!("set_text: {e}"))?;

    paste_key_combo()?;

    // dá tempo do app alvo processar a colagem antes de restaurar
    std::thread::sleep(std::time::Duration::from_millis(300));
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

fn type_text(text: &str) -> Result<(), String> {
    use enigo::{Enigo, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.text(text).map_err(|e| format!("enigo.text: {e}"))?;
    Ok(())
}
