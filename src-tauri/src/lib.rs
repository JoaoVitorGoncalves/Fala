// Fala — ditador local push-to-talk (macOS + Windows).
// Segure o atalho global → grava; solte → transcreve com whisper.cpp local → cola no app focado.

mod audio;
mod inject;
mod live;
mod llm;
mod settings;
mod stt;

use device_query::DeviceQuery;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
    /// configuração congelada no início: salvar settings durante a sessão
    /// nunca muda o destino dos trechos já capturados
    meeting_mode: bool,
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
    /// polimento async: fila de segmentos + worker próprio (não bloqueia a injeção)
    polish_queue: Mutex<Vec<PendingPolish>>,
    polish_next_id: AtomicU64,
    /// painel "versão limpa": segmentos acumulados (cru até chegar o limpo)
    panel: Mutex<Vec<PanelSegment>>,
    panel_generation: AtomicU64,
    /// chars colados por trecho, na ordem — base dos comandos "apagar"
    injected: Mutex<Vec<usize>>,
    /// prefixo de comando que atravessou a fronteira entre dois chunks
    command_buffer: Mutex<String>,
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

/// Polimento IA com as guardas determinísticas. Devolve o texto limpo ou None
/// (resposta/recusa do modelo, saída vazia ou falha de rede) — o chamador
/// mantém o texto bruto nesses casos. Nunca bloqueia nada além do chamador.
fn polish_checked(ai: &settings::AiSettings, text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let sys = ai
        .system_prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| llm::default_system_prompt().into());
    let started = Instant::now();
    match llm::polish(ai, &sys, text) {
        Ok(p) if !p.trim().is_empty() => {
            // Salvaguarda determinística: correção legítima tem tamanho ~igual
            // ao ditado e mesma estrutura. Saída muito maior (ou com markdown
            // que o ditado não tinha) é o modelo RESPONDENDO/completando um
            // fragmento — descarta.
            let inp = text.chars().count();
            let out = p.chars().count();
            let sim = llm::similarity(text.trim(), p.trim());
            let tem_lista = |t: &str| {
                t.lines().any(|l| {
                    let l = l.trim_start();
                    l.starts_with("- ") || l.starts_with("* ") || l.starts_with("• ")
                })
            };
            if out > inp * 2 + 30 || (tem_lista(&p) && !tem_lista(text)) || sim < 0.6 {
                diag(&format!(
                    "polimento IA descartado (resposta/recusa; similaridade {sim:.2}): {p}"
                ));
                None
            } else {
                diag(&format!(
                    "polimento IA em {:.2}s: {p}",
                    started.elapsed().as_secs_f32()
                ));
                Some(p)
            }
        }
        Ok(_) => {
            diag("polimento IA retornou vazio; mantendo o texto bruto");
            None
        }
        Err(e) => {
            diag(&format!(
                "polimento IA falhou ({e}); mantendo o texto bruto"
            ));
            None
        }
    }
}

/// Segmento pendente de polimento (ao vivo: um trecho commitado; final: a cauda).
#[derive(Debug, Clone)]
pub(crate) struct PendingPolish {
    pub(crate) id: u64,
    pub(crate) text: String,
    pub(crate) panel_generation: u64,
}

/// Segmento do painel "Versão limpa": cru até o worker devolver o texto limpo.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PanelSegment {
    pub(crate) id: u64,
    pub(crate) raw: String,
    pub(crate) clean: Option<String>,
}

/// Adiciona texto cru ao painel, independentemente da IA estar disponível.
/// Enfileira um texto para polimento async. O painel recebe o raw mesmo sem IA;
/// só a etapa de limpeza depende do servidor.
pub(crate) fn enqueue_polish(app: &AppHandle, text: String) {
    if text.trim().is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let ai_on = state
        .settings
        .lock()
        .unwrap()
        .ai
        .as_ref()
        .is_some_and(|a| a.enabled);
    let id = state.polish_next_id.fetch_add(1, Ordering::Relaxed);
    let panel_generation = state.panel_generation.load(Ordering::Acquire);
    state.panel.lock().unwrap().push(PanelSegment {
        id,
        raw: text.clone(),
        clean: None,
    });
    emit_panel(app);
    if !ai_on {
        diag("painel: IA desligada; mantendo segmento cru");
        return;
    }
    state.polish_queue.lock().unwrap().push(PendingPolish {
        id,
        text,
        panel_generation,
    });
}

/// Estado completo do painel para as janelas (broadcast — o painel re-renderiza).
fn emit_panel(app: &AppHandle) {
    let segments = app.state::<AppState>().panel.lock().unwrap().clone();
    let _ = app.emit("panel-state", serde_json::json!({ "segments": segments }));
}

/// Aplica o texto limpo ao painel, colapsando o bloco de segmentos [first..=last]
/// (lote mesclado vira um segmento só).
fn panel_apply_clean(
    app: &AppHandle,
    first_id: u64,
    last_id: u64,
    raw_merged: &str,
    clean: &str,
    generation: u64,
) {
    let state = app.state::<AppState>();
    if state.panel_generation.load(Ordering::Acquire) != generation {
        diag("painel: resultado antigo descartado após limpar");
        return;
    }
    let mut segs = state.panel.lock().unwrap();
    if let Some(pos) = segs.iter().position(|s| s.id == first_id) {
        let end = segs[pos..].iter().take_while(|s| s.id <= last_id).count();
        segs.splice(
            pos..pos + end,
            [PanelSegment {
                id: first_id,
                raw: raw_merged.to_string(),
                clean: Some(clean.to_string()),
            }],
        );
    }
    drop(segs);
    emit_panel(app);
}

/// Worker do polimento async: drena a fila em background — nunca toca o
/// caminho da injeção. Backlog ≥ 3 colapsa num request só (o servidor é
/// single-user; o resultado vale para o lote todo). Emite `clean-version`.
fn spawn_polish_worker(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let batch: Vec<PendingPolish> = {
                let state = app.state::<AppState>();
                let mut q = state.polish_queue.lock().unwrap();
                if q.is_empty() {
                    continue;
                }
                let take = q.len().min(if q.len() >= 3 { q.len() } else { 1 });
                q.drain(..take).collect()
            };
            let Some(ai) = app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .ai
                .clone()
                .filter(|a| a.enabled)
            else {
                continue; // IA desligou no meio: lote descartado (texto bruto já foi entregue)
            };
            // colapso: muitos pendentes viram um texto único polido de uma vez
            let batch_ids: Vec<u64> = batch.iter().map(|b| b.id).collect();
            let items = if batch.len() > 1 {
                let id = batch[0].id;
                let text = batch
                    .iter()
                    .map(|p| p.text.trim().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                vec![PendingPolish {
                    id,
                    text,
                    panel_generation: batch[0].panel_generation,
                }]
            } else {
                batch
            };
            for p in items {
                if let Some(clean) = polish_checked(&ai, &p.text) {
                    // lote mesclado colapsa todos os ids do lote [first..=last] num segmento
                    let first = *batch_ids.first().unwrap_or(&p.id);
                    let last = *batch_ids.last().unwrap_or(&p.id);
                    panel_apply_clean(&app, first, last, &p.text, &clean, p.panel_generation);
                    let _ = app.emit(
                        "clean-version",
                        serde_json::json!({ "id": p.id, "text": clean }),
                    );
                }
            }
        }
    });
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
    // Cada gravação tem sua própria pilha de edição/comandos. O painel, por
    // outro lado, continua acumulando entre sessões.
    state.injected.lock().unwrap().clear();
    state.command_buffer.lock().unwrap().clear();

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
    diag(&format!(
        "gravando em: {}",
        settings
            .device
            .as_deref()
            .unwrap_or("entrada padrão do sistema")
    ));
    // transcrição em tempo real: worker próprio confirmando trechos a cada
    // pausa. No macOS (Metal) os trechos usam o modelo grande — latência de
    // small-na-CPU com precisão do large; em CPU pura (Windows), o modelo
    // rápido dedicado. Sem modelo/sem live: gravação segue como antes.
    // O modelo rápido configurado/baixado é válido nos dois SOs. Só cai no
    // modelo principal se o small ainda não existir, para não deixar o live
    // silenciosamente sem worker.
    let live_model =
        settings::live_model_path_or_default(settings).or_else(|| settings.model_path.clone());
    let live = match live_model {
        Some(path) if settings.live_transcribe => Some(live::spawn(
            app.clone(),
            recorder.handle(),
            path,
            settings.language.clone(),
            settings.vocab_prompt.clone(),
            settings.corrections.clone(),
            generation,
            settings.meeting_mode,
        )),
        _ => None,
    };
    *state.recording.lock().unwrap() = Some(Recording {
        recorder,
        started: Instant::now(),
        hud_generation: generation,
        meeting_mode: settings.meeting_mode,
        live,
    });
    state.set_status(Status::Recording);
    if settings.meeting_mode {
        update_tray(
            app,
            Status::Recording,
            Some("Fala — reunião: capturando sem colar"),
        );
    } else {
        update_tray(app, Status::Recording, None);
    }
    hud_state(app, "recording", None);
    show_hud(app);
    if settings.meeting_mode {
        hud_chip(app, "Reunião: capturando sem colar");
    }

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
    std::thread::spawn(move || {
        loop {
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
                let pressed =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| kb.get_keys()));
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
                schedule_hud_hide(&handle, 3000, state.hud_generation.load(Ordering::Relaxed));
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
    let meeting_mode = recording.meeting_mode;
    let live_handle = recording.live;
    let had_live_worker = live_handle.is_some();
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
    // "silêncio digital" nos últimos 4s: ~100% = o stream entregou zeros
    // (trava do dispositivo/BT) mesmo com o usuário falando; ruído ambiente
    // real fica bem abaixo disso. Fala no começo + zeros no fim = mic travou.
    let cauda = &samples[samples.len().saturating_sub(64_000)..];
    let zeros = cauda.iter().filter(|s| s.abs() < 1e-4).count();
    let zeros_pct = zeros as f32 / cauda.len().max(1) as f32 * 100.0;
    let had_speech = níveis.iter().any(|rms| *rms > 0.015);
    let mic_stalled = had_speech && zeros_pct > 95.0;
    diag(&format!(
        "silêncio digital nos últimos 4s: {zeros_pct:.0}%{}",
        if mic_stalled { " → MIC TRAVOU" } else { "" }
    ));

    let settings = state.settings.lock().unwrap().clone();
    let model_path = settings.model_path.unwrap_or_default();
    let language = settings.language.clone();
    let paste_mode = settings.paste_mode.clone();
    let vocab_prompt = settings
        .vocab_prompt
        .clone()
        .filter(|p| !p.trim().is_empty());
    let corrections = settings.corrections.clone();
    let ai = settings.ai.clone();
    // comportamento clássico opt-in: esperar o polimento IA antes de colar
    let polish_before_paste = ai
        .as_ref()
        .is_some_and(|a| a.enabled && a.polish_before_paste);
    // Usa a configuração congelada quando a gravação começou. Não reler o
    // settings aqui: salvar durante uma sessão não pode mudar seu destino.
    let app = app.clone(); // handle próprio para a thread de transcrição
    std::thread::spawn(move || {
        // Aguarda o worker ao vivo. O áudio parado completo é a fonte de
        // verdade: o último tick/decode pode ter deixado amostras fora do
        // snapshot do worker.
        let live = match live_handle {
            Some(h) => match h.join() {
                Ok(r) => r,
                Err(_) => {
                    diag("live: worker encerrou com panic; transcrevendo o áudio completo");
                    live::LiveResult {
                        committed_text: String::new(),
                        committed_samples: 0,
                    }
                }
            },
            None => {
                diag("live: worker ausente; transcrevendo o áudio completo");
                live::LiveResult {
                    committed_text: String::new(),
                    committed_samples: 0,
                }
            }
        };

        // O worker live pode ter parado no último tick antes de o Recorder ser
        // drenado. Reconcilia sempre contra `samples`, nunca contra sua cauda
        // parcial. Sem worker (ou após panic), a sessão inteira é a cauda.
        let committed_samples = if had_live_worker {
            live.committed_samples.min(samples.len())
        } else {
            0
        };
        let tail = samples[committed_samples..].to_vec();
        diag(&format!(
            "reconciliação: meeting={meeting_mode}, live_worker={had_live_worker}, committed_samples={committed_samples}, tail_samples={}",
            tail.len()
        ));

        let started = Instant::now();
        // Revisão rápida: só a cauda aberta passa pelo modelo grande. Sem live
        // (ou tudo já confirmado), a cauda é o ditado inteiro — comportamento
        // clássico. Correções do usuário e o polimento IA se aplicam ao resultado.
        let result = if tail.is_empty() {
            let pending = take_command_buffer(&app);
            if !pending.is_empty() {
                diag(&format!("comando pendente liberado como texto: {pending}"));
                Ok(if live.committed_text.is_empty() {
                    pending
                } else {
                    format!("{} {pending}", live.committed_text.trim())
                })
            } else {
                Ok(live.committed_text.clone())
            }
        } else {
            app.state::<AppState>()
                .transcriber
                .transcribe(&tail, &model_path, &language, vocab_prompt.as_deref())
                .map(|text| apply_corrections(text, &corrections))
                .map(|text| merge_command_buffer(&app, text))
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
                if let Some(cmd) = detect_command(&text) {
                    // comando dito na cauda (ou sem live): vira ação, não texto
                    match execute_command(&app, cmd) {
                        Ok(feedback) => {
                            hud_chip(&app, feedback);
                            serde_json::json!({ "ok": true, "text": format!("[{feedback}]") })
                        }
                        Err(e) => {
                            hud_chip(&app, &format!("Comando falhou: {e}"));
                            serde_json::json!({ "ok": true, "text": text })
                        }
                    }
                } else if tail.is_empty() && text == live.committed_text {
                    // tudo já foi digitado pelos commits ao vivo
                    serde_json::json!({ "ok": true, "text": text })
                } else if meeting_mode {
                    // modo reunião: nada é colado — a cauda vai só para o painel
                    enqueue_polish(&app, text.clone());
                    let full = if live.committed_text.is_empty() {
                        text
                    } else {
                        format!("{} {text}", live.committed_text.trim())
                    };
                    serde_json::json!({ "ok": true, "text": full, "meeting": true })
                } else {
                    // Comportamento clássico opt-in: esperar a IA e colar polido.
                    // Padrão: cola rápido (correções locais) e o polimento segue
                    // async para o painel/evento clean-version.
                    let text = if polish_before_paste {
                        let st = app.state::<AppState>();
                        if st.hud_generation.load(Ordering::Relaxed) == generation
                            && st.status() == Status::Transcribing
                        {
                            hud_state(&app, "transcribing", Some("Polindo com IA…"));
                        }
                        ai.as_ref()
                            .and_then(|a| polish_checked(a, &text))
                            .unwrap_or(text)
                    } else {
                        text
                    };
                    // insere a cauda revisada — nada existente é apagado
                    let sep = if live.committed_text.is_empty() {
                        ""
                    } else {
                        " "
                    };
                    let injected = format!("{sep}{text}");
                    match inject::paste_text(&app, &injected, &paste_mode) {
                        Ok(()) => {
                            if !polish_before_paste {
                                // versão limpa async: a cauda cru vai para o worker
                                enqueue_polish(&app, text.clone());
                            }
                            // rastreia chars colados (base dos comandos "apagar")
                            app.state::<AppState>()
                                .injected
                                .lock()
                                .unwrap()
                                .push(injected.chars().count());
                            serde_json::json!({ "ok": true, "text": text })
                        }
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
                    diag(&format!(
                        "erro na revisão da cauda ({e}); mantendo trechos confirmados"
                    ));
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
            if mic_stalled {
                // o stream entregou zeros no fim: avisar em vez de sucesso silencioso
                hud_state(
                    &app,
                    "error",
                    Some(
                        "Microfone travou durante a gravação (Bluetooth?) — texto até aqui foi entregue",
                    ),
                );
                schedule_hud_hide(&app, 3400, generation);
            } else {
                hud_state(&app, "done", None);
                schedule_hud_hide(&app, 1400, generation);
            }
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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
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

// ---------------------------------------------------------------- painel flutuante

const PANEL_W: f64 = 340.0;
const PANEL_H: f64 = 520.0;

/// Posiciona o painel encostado na borda direita, verticalmente centrado.
fn position_panel(app: &AppHandle) {
    let Some(win) = app.get_webview_window("panel") else {
        return;
    };
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let sf = monitor.scale_factor();
        let mpos = monitor.position();
        let msize = monitor.size();
        let w = (PANEL_W * sf).round() as i32;
        let h = (PANEL_H * sf).round() as i32;
        let x = mpos.x + msize.width as i32 - w - (24.0 * sf).round() as i32;
        let y = mpos.y + (msize.height as i32 - h) / 2;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Mostra/esconde o painel "versão limpa" (item da tray).
fn toggle_panel(app: &AppHandle) {
    let Some(win) = app.get_webview_window("panel") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        position_panel(app);
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
    if state.shutdown_waiter_started.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            let state = handle.state::<AppState>();
            if state.active_transcriptions.load(Ordering::Acquire) == 0 {
                handle.exit(0);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
    gs.unregister_all()
        .map_err(|e| format!("unregister_all: {e}"))?;
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
    let previous = app.state::<AppState>().settings.lock().unwrap().clone();
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

/// Conflito HFP: mic Bluetooth selecionado com fone BT na saída degrada a
/// música ("efeito copo") enquanto o Fala grava. None = sem conflito.
#[tauri::command]
fn audio_conflict(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    let device = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .device
        .clone();
    Ok(audio::bt_conflict(device.as_deref())
        .map(|c| serde_json::json!({ "input": c.input_name, "suggestion": c.suggestion })))
}

// ---------------------------------------------------------------- painel (versão limpa)

#[tauri::command]
fn panel_state(app: AppHandle) -> Vec<PanelSegment> {
    app.state::<AppState>().panel.lock().unwrap().clone()
}
/// Copia a versão limpa (ou o cru, onde o polimento ainda não chegou) e devolve
/// o número de caracteres copiados.
#[tauri::command]
fn panel_copy(app: AppHandle) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let segs = state.panel.lock().unwrap().clone();
    let text = segs
        .iter()
        .map(|s| s.clean.clone().unwrap_or_else(|| s.raw.clone()))
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        return Err("painel vazio — nada para copiar".into());
    }
    let n = text.chars().count();
    inject::copy_to_clipboard(&text)?;
    Ok(n)
}

#[tauri::command]
fn panel_clear(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.panel_generation.fetch_add(1, Ordering::AcqRel);
    state.polish_queue.lock().unwrap().clear();
    state.panel.lock().unwrap().clear();
    emit_panel(&app);
    Ok(())
}

// ---------------------------------------------------------------- comandos de voz

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceCommand {
    NewParagraph,
    DeleteAll,
    DeleteLast,
    CopyClean,
}

/// Normaliza para comparação: minúsculas, sem acentos, sem pontuação final.
fn normalize_command(text: &str) -> String {
    // Whisper pode variar pontuação e espaços entre chunks. Transformar toda
    // pontuação em separador evita que "novo, parágrafo" deixe de ser comando;
    // depois split_whitespace torna múltiplos espaços equivalentes a um.
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const VOICE_COMMANDS: &[(&str, VoiceCommand)] = &[
    ("novo paragrafo", VoiceCommand::NewParagraph),
    ("apagar tudo", VoiceCommand::DeleteAll),
    ("apagar ultimo", VoiceCommand::DeleteLast),
    ("copiar versao limpa", VoiceCommand::CopyClean),
];

/// Frase exata = comando. O texto do comando nunca é colado nem vai ao painel.
pub(crate) fn detect_command(text: &str) -> Option<VoiceCommand> {
    let normalized = normalize_command(text);
    VOICE_COMMANDS
        .iter()
        .find(|(phrase, _)| *phrase == normalized)
        .map(|(_, command)| *command)
}

fn is_command_prefix(text: &str) -> bool {
    let normalized = normalize_command(text);
    !normalized.is_empty()
        && VOICE_COMMANDS
            .iter()
            .any(|(phrase, _)| phrase.starts_with(&(normalized.clone() + " ")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandDecision {
    Text(String),
    Pending,
    Execute(VoiceCommand),
}

/// Decide um chunk sem perder frases divididas pelo worker live. Só segura um
/// texto que é prefixo exato de comando; fala normal é liberada imediatamente.
fn command_decision_with_buffer(buffer: &mut String, text: &str) -> CommandDecision {
    let candidate = if buffer.trim().is_empty() {
        text.trim().to_string()
    } else {
        format!("{} {}", buffer.trim(), text.trim())
    };
    if let Some(command) = detect_command(&candidate) {
        buffer.clear();
        return CommandDecision::Execute(command);
    }
    if is_command_prefix(&candidate) {
        *buffer = candidate;
        return CommandDecision::Pending;
    }
    let output = if buffer.trim().is_empty() {
        text.trim().to_string()
    } else {
        candidate
    };
    buffer.clear();
    CommandDecision::Text(output)
}

pub(crate) fn command_decision(app: &AppHandle, text: &str) -> CommandDecision {
    let state = app.state::<AppState>();
    command_decision_with_buffer(&mut state.command_buffer.lock().unwrap(), text)
}

/// Junta prefixo retido com a cauda final. O prefixo só foi retido porque a
/// gravação ainda podia completar um comando no stop.
pub(crate) fn take_command_buffer(app: &AppHandle) -> String {
    let state = app.state::<AppState>();
    let mut buffer = state.command_buffer.lock().unwrap();
    let output = buffer.trim().to_string();
    buffer.clear();
    output
}

fn merge_command_buffer(app: &AppHandle, text: String) -> String {
    let pending = take_command_buffer(app);
    if pending.is_empty() {
        text
    } else if text.trim().is_empty() {
        pending
    } else {
        format!("{pending} {}", text.trim())
    }
}

/// Feedback leve no HUD sem mudar de estado (gravação continua).
pub(crate) fn hud_chip(app: &AppHandle, message: &str) {
    let _ = app.emit("hud-chip", serde_json::json!({ "message": message }));
}

/// Executa um comando de voz. O texto colado pelo Fala é rastreado em
/// `AppState.injected` (chars por trecho) — é isso que os backspaces apagam.
pub(crate) fn execute_command(app: &AppHandle, cmd: VoiceCommand) -> Result<&'static str, String> {
    let state = app.state::<AppState>();
    match cmd {
        VoiceCommand::NewParagraph => {
            inject::enter_on_main(app, 2)?;
            Ok("Parágrafo novo ✓")
        }
        VoiceCommand::DeleteAll => {
            let total: usize = state.injected.lock().unwrap().iter().sum();
            inject::backspace_on_main(app, total)?;
            state.injected.lock().unwrap().clear();
            Ok("Texto apagado ✓")
        }
        VoiceCommand::DeleteLast => {
            let last = state.injected.lock().unwrap().pop().unwrap_or(0);
            if last == 0 {
                return Err("nada para apagar".into());
            }
            inject::backspace_on_main(app, last)?;
            Ok("Trecho apagado ✓")
        }
        VoiceCommand::CopyClean => {
            let segs = state.panel.lock().unwrap().clone();
            let text = segs
                .iter()
                .map(|s| s.clean.clone().unwrap_or_else(|| s.raw.clone()))
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                return Err("versão limpa está vazia".into());
            }
            inject::copy_to_clipboard(&text)?;
            Ok("Versão limpa copiada ✓")
        }
    }
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
        app.state::<AppState>()
            .live_download
            .store(false, Ordering::Release);
        return Err("variável HOME não definida".into());
    };
    let dir = std::path::Path::new(&home).join(".fala/models");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        app.state::<AppState>()
            .live_download
            .store(false, Ordering::Release);
        return Err(format!("create_dir_all: {e}"));
    }
    let final_path = dir.join(settings::LIVE_MODEL_FILE);
    if final_path.exists() {
        app.state::<AppState>()
            .live_download
            .store(false, Ordering::Release);
        return Ok(());
    }
    let part_path = dir.join(format!("{}.part", settings::LIVE_MODEL_FILE));

    // progresso: tamanho do .part enquanto o curl roda (metadata falhou = fim)
    let prog_app = app.clone();
    let prog_part = part_path.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let Ok(meta) = std::fs::metadata(&prog_part) else {
                break;
            };
            let _ = prog_app.emit(
                "model-download-progress",
                serde_json::json!({ "received": meta.len() }),
            );
        }
    });

    let dl_app = app.clone();
    std::thread::spawn(move || {
        let url = format!(
            "{}/{}",
            settings::LIVE_MODEL_BASE,
            settings::LIVE_MODEL_FILE
        );
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

#[cfg(test)]
mod tests {
    use super::{
        CommandDecision, VoiceCommand, command_decision_with_buffer, detect_command,
        normalize_command,
    };

    #[test]
    fn commands_normalize_accents_punctuation_and_spaces() {
        assert_eq!(normalize_command("  NOVO,   parágrafo! "), "novo paragrafo");
        assert_eq!(
            detect_command("novo parágrafo"),
            Some(VoiceCommand::NewParagraph)
        );
        assert_eq!(
            detect_command("APAGAR último."),
            Some(VoiceCommand::DeleteLast)
        );
        assert_eq!(
            detect_command("copiar versão limpa?"),
            Some(VoiceCommand::CopyClean)
        );
    }

    #[test]
    fn normal_text_is_not_a_command() {
        assert_eq!(detect_command("novo parágrafo agora"), None);
        assert_eq!(detect_command("eu preciso apagar tudo depois"), None);
    }

    #[test]
    fn command_prefix_survives_chunk_boundary() {
        let mut buffer = String::new();
        assert_eq!(
            command_decision_with_buffer(&mut buffer, "novo"),
            CommandDecision::Pending
        );
        assert_eq!(buffer, "novo");
        assert_eq!(
            command_decision_with_buffer(&mut buffer, "parágrafo"),
            CommandDecision::Execute(VoiceCommand::NewParagraph)
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn prefix_that_stops_being_command_is_released_as_text() {
        let mut buffer = String::new();
        assert_eq!(
            command_decision_with_buffer(&mut buffer, "apagar"),
            CommandDecision::Pending
        );
        assert_eq!(
            command_decision_with_buffer(&mut buffer, "a reunião"),
            CommandDecision::Text("apagar a reunião".into())
        );
    }
}

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
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
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
            audio_conflict,
            panel_state,
            panel_copy,
            panel_clear,
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
                polish_queue: Mutex::new(Vec::new()),
                polish_next_id: AtomicU64::new(0),
                panel: Mutex::new(Vec::new()),
                panel_generation: AtomicU64::new(0),
                injected: Mutex::new(Vec::new()),
                command_buffer: Mutex::new(String::new()),
            });

            // versão limpa: worker de polimento async (drena a fila em background)
            spawn_polish_worker(app.handle().clone());

            let show = MenuItem::with_id(app, "settings", "Configurações…", true, None::<&str>)?;
            let panel = MenuItem::with_id(app, "panel", "Versão limpa", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &panel, &quit])?;

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().expect("ícone padrão").clone())
                .tooltip("Fala — pronto")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| {
                    eprintln!("[fala] menu da bandeja: {}", ev.id.as_ref());
                    match ev.id.as_ref() {
                        "settings" => show_settings_window(app),
                        "panel" => toggle_panel(app),
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
            if settings.ai.clone().is_some_and(|ai| {
                ai.enabled && ai.sync_profile && !ai.profile_url.trim().is_empty()
            }) {
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
                settings
                    .model_path
                    .as_deref()
                    .unwrap_or("(não configurado)")
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
