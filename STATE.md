# Fala — estado do projeto (handoff)

> Documento canônico de estado. Atualizar a cada entrega relevante.

## O que é
Ditador local push-to-talk/toggle para macOS+Windows, estilo Wispr Flow, 100% on-device.
Stack: **Tauri 2 + Rust + React 19 + TS** (Vite), repo em `~/Projects/fala`, bundle id `dev.fala.app`.

## Funcionalidades entregues (commit head: ver `git log --oneline -1`)
- Atalho global (capturador na UI; padrão `alt+space` = ⌥ Option+Espaço no Mac)
- **Modo toggle** (padrão): aperta uma vez, auto-stop por silêncio (VAD por energia, `silence_ms` configurável); **modo push**: segurar
- **Timeout sem fala (7s)**: mic mudo/indisponível descarta a gravação com aviso no HUD (não fica "ouvindo infinito")
- **Esc finaliza** a gravação (polling `device_query` na thread de supervisão)
- Whisper.cpp local (Metal), modelo `large-v3-turbo-q5_0` (auto-detectado de `~/.zcode/voice-poc/models/`)
- Qualidade de transcrição: trim de silêncio das pontas, reticências limpas, **vocabulário personalizado** (prompt inicial) e **correções automáticas** (`errado = correto` por linha)
- Inserção via clipboard (com restauração) ou digitação; **enigo/arboard SEMPRE na main thread** (crash HIToolbox/TIS se não)
- Tray 3 estados (● rec / … txt); HUD flutuante não-focável com waveform ao vivo, spinner, ✓ animado
- Diagnóstico: mapa de níveis (RMS 2s) + eventos do atalho no log; panics em `~/.fala/panic.log`

## Arquitetura (src-tauri/src/)
- `lib.rs` — bootstrap, tray, máquina de estados (Idle/Recording/Transcribing), watcher (níveis + auto-stops + Esc), comandos Tauri
- `audio.rs` — cpal capture → 16 kHz mono (downmix + resample linear), `VadState` (rms/speech_seen/last_voice)
- `stt.rs` — whisper-rs 0.16 (contexto persistente, Metal), trim de silêncio, clean_text
- `inject.rs` — paste/insert; `on_main()` para enigo (HIToolbox exige main thread)
- `settings.rs` — JSON em app_config_dir, `#[serde(default)]`

## Pendências / issues abertas
1. **Identidade estável "Fala Dev" — RESOLVIDA**: cert autoassinado criado via Certificate Assistant (GUI) no login keychain; `scripts/sign.sh` re-assina o bundle pós-build (assinar por NOME — `find-identity -p codesigning` não lista autoassinados, mas o codesign usa). PRIMEIRO uso: usuário concedeu mic/Acessibilidade uma última vez; agora persiste entre rebuilds. ⚠️ `pnpm tauri build` assina ad-hoc por cima → SEMPRE rodar `scripts/sign.sh` depois.
2. **Crash device_query resolvido**: device_query faz `panic!` sem Acessibilidade (macos/mod.rs:122); monitor de teclado só é criado com permissão confirmada + `catch_unwind` em cada `get_keys()`.
3. **DESIGN + ESTABILIDADE (iter. 5 — implementado e buildado)** — usuário pediu acabamento de app público e reportou que o HUD parecia não mudar/encolher entre estados. Diagnóstico: o bundle release precisa ser fechado e reconstruído; além disso, o shell escalava em `hud-in`/`hud-leaving`, os estados tinham footprints intrínsecos diferentes e timers antigos podiam aplicar fade/hide a uma gravação nova. Mudanças visuais: HUD 320×84 (Rust + `tauri.conf.json` sincronizados), Liquid Glass `liquidGlassClear` radius 42, sem tint preto, shell sem escala (apenas opacity), contrato explícito `html/body/#root` 100%, slot fixo 252×52, transições internas fade/translate/blur, timer de gravação discreto, spinner conic, scan sutil no estado recording; waveform original mantida (38 barras + dot vermelho). Mudanças de ciclo: timeout React cancelável; `hud_generation: AtomicU64`; hides/worker stale guardados por geração/status; erro de inicialização mostra HUD. **Crash ao fechar investigado:** 18 relatórios macOS mostram duas famílias — SIGTRAP antigo do HIToolbox/enigo (já corrigido) e SIGABRT recente durante `app.exit`/AppKit teardown; evidência forte aponta para assert de residency Metal do Whisper/ggml ao destruir `WhisperContext` persistente (`[rsets->data count] == 0`), agravado por workers detached. Mitigação aplicada: `use_gpu: !cfg!(target_os = "macos")` (CPU no macOS, GPU preservada no Windows), `shutting_down`/`active_transcriptions`/waiter no `AppState`, parada do recorder, unregister do global shortcut, bloqueio de novos workers, espera de transcrição ativa antes de `app.exit`, tratamento de `ExitRequested`. Checks: `pnpm build`, `cargo check`, JSON config e harness visual passaram. Build Tauri final concluído, assinado com `Fala Dev` e aberto (PID 98822). Computer Use broker não ficou disponível; processo final foi confirmado vivo. Mudanças ainda não commitadas.
4. Fase 2: polimento IA (Ollama/cloud), streaming, ícone do app, assinatura Windows, auto-update.

## Constraints técnicos que NÃO podem regredir
- enigo/arboard/tray: sempre via `run_on_main_thread` onde indicado
- HUD: `set_focusable(false)` (canBecomeKeyWindow=false) — NÃO roubar foco do app alvo
- Sem rede: tudo local
- `#[serde(default)]` em Settings; `panic = "unwind"` + hook gravando `~/.fala/panic.log`

## Build/loop
`pnpm tauri build` → `scripts/sign.sh` (quando cert existir) → `open .../Fala.app`.
Logs ao vivo: lançar `.../Fala.app/Contents/MacOS/fala` pelo terminal.
Crash reports: `~/Library/Logs/DiagnosticReports/fala-*.ips` (parser python já usado na sessão).

## BRIEF DE DESIGN (tarefa atual)
Objetivo: parecer **app nativo da Apple (macOS 26, Liquid Glass)**. Usuário rejeitou a tentativa 1 ("não mudou quase nada" — cards opacos cobrindo o vibrancy, hierarquia de lista agrupada inexistente, controles estilo web, falta de ícones leading).
Referência de linguagem: Ajustes do Sistema (lista agrupada inset com linhas+separadores, ícones leading em quadrados arredondados, headers de seção uppercase), HUD estilo material hudWindow.
Entregáveis do agente: tokens exatos (light/dark), layout completo, App.css + Hud.css novos prontos, diffs pontuais de TSX, checklist visual.
Skills a carregar pelo agente: `design-taste-frontend`, `high-end-visual-design`.
