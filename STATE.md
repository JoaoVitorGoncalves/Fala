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
- **Transcrição em tempo real por COMMITS (branch `feature/transcricao-tempo-real`, v2 do desenho)**: worker `live.rs` fecha um trecho a cada pausa da fala (≥700ms; teto de 9s p/ fala contínua, corte no ponto mais silencioso) e o decodifica UMA vez com modelo rápido dedicado (`small-q5_1` — small é q5_1 no repo whisper.cpp, q5_0 dá 404; CPU, contexto próprio) → digita como texto FINAL (sem re-decode, sem backspace). Ao soltar, SÓ a cauda aberta é revisada pelo modelo grande + correções e digitada. Prompt do trecho = vocabulário + cauda do texto confirmado. Sem live/sem modelo → comportamento clássico intacto. Modelo baixável pela UI (`download_live_model` → `~/.fala/models/ggml-small-q5_1.bin`, ~181 MB, curl + `.part` + rename atômico). v1 (provisório + substituição final com re-decode completo e diff/backspace) foi descartada: travava em falas longas (re-decodificava tudo a cada ~1,2s), reescrevia o texto no fim e misturava sessões — feedback direto do usuário.
- Inserção via clipboard (com restauração) ou digitação; **enigo/arboard SEMPRE na main thread** (crash HIToolbox/TIS se não)
- Tray 3 estados (● rec / … txt); HUD flutuante não-focável com waveform ao vivo, spinner, ✓ animado
- Diagnóstico: mapa de níveis (RMS 2s) + eventos do atalho no log; panics em `~/.fala/panic.log`

## Arquitetura (src-tauri/src/)
- `lib.rs` — bootstrap, tray, máquina de estados (Idle/Recording/Transcribing), watcher (níveis + auto-stops + Esc), comandos Tauri, `apply_corrections`
- `audio.rs` — cpal capture → 16 kHz mono (downmix + resample linear), `VadState` (rms/speech_seen/last_voice), `RecorderHandle` (snapshot não-destrutivo p/ o live)
- `stt.rs` — whisper-rs 0.16 (contexto persistente, Metal), trim de silêncio, clean_text, `base_params` compartilhado
- `live.rs` — worker de commits ao vivo (ctx pequeno dedicado, `WhisperState` reusado, digitação via `inject`)
- `inject.rs` — paste/insert; `on_main()` para enigo (HIToolbox exige main thread), `type_text_on_main`
- `settings.rs` — JSON em app_config_dir, `#[serde(default)]`, `live_transcribe`/`live_model_path` + `live_model_path_or_default`

## Pendências / issues abertas
1. **Identidade estável "Fala Dev" — RESOLVIDA**: cert autoassinado criado via Certificate Assistant (GUI) no login keychain; `scripts/sign.sh` re-assina o bundle pós-build (assinar por NOME — `find-identity -p codesigning` não lista autoassinados, mas o codesign usa). PRIMEIRO uso: usuário concedeu mic/Acessibilidade uma última vez; agora persiste entre rebuilds. ⚠️ `pnpm tauri build` assina ad-hoc por cima → SEMPRE rodar `scripts/sign.sh` depois.
2. **Crash device_query resolvido**: device_query faz `panic!` sem Acessibilidade (macos/mod.rs:122); monitor de teclado só é criado com permissão confirmada + `catch_unwind` em cada `get_keys()`.
3. **DESIGN + ESTABILIDADE (iter. 5 — implementado e buildado)** — usuário pediu acabamento de app público e reportou que o HUD parecia não mudar/encolher entre estados. Diagnóstico: o bundle release precisa ser fechado e reconstruído; além disso, o shell escalava em `hud-in`/`hud-leaving`, os estados tinham footprints intrínsecos diferentes e timers antigos podiam aplicar fade/hide a uma gravação nova. Mudanças visuais: HUD 320×84 (Rust + `tauri.conf.json` sincronizados), Liquid Glass `liquidGlassClear` radius 42, sem tint preto, shell sem escala (apenas opacity), contrato explícito `html/body/#root` 100%, slot fixo 252×52, transições internas fade/translate/blur, timer de gravação discreto, spinner conic, scan sutil no estado recording; waveform original mantida (38 barras + dot vermelho). Mudanças de ciclo: timeout React cancelável; `hud_generation: AtomicU64`; hides/worker stale guardados por geração/status; erro de inicialização mostra HUD. **Crash ao fechar investigado:** 18 relatórios macOS mostram duas famílias — SIGTRAP antigo do HIToolbox/enigo (já corrigido) e SIGABRT recente durante `app.exit`/AppKit teardown; evidência forte aponta para assert de residency Metal do Whisper/ggml ao destruir `WhisperContext` persistente (`[rsets->data count] == 0`), agravado por workers detached. Mitigação aplicada: `use_gpu: !cfg!(target_os = "macos")` (CPU no macOS, GPU preservada no Windows), `shutting_down`/`active_transcriptions`/waiter no `AppState`, parada do recorder, unregister do global shortcut, bloqueio de novos workers, espera de transcrição ativa antes de `app.exit`, tratamento de `ExitRequested`. Checks: `pnpm build`, `cargo check`, JSON config e harness visual passaram. Build Tauri final concluído, assinado com `Fala Dev` e aberto (PID 98822). Computer Use broker não ficou disponível; processo final foi confirmado vivo. Mudanças ainda não commitadas.
4. Fase 2: polimento IA (Ollama/cloud), ícone do app, assinatura Windows, auto-update. Live por commits **implementado na `feature/transcricao-tempo-real`** — validar em uso real: cadência dos commits no log (`live: trecho confirmado …`), ausência de travas em falas longas, texto antigo intocado entre ditados.

## Constraints técnicos que NÃO podem regredir
- enigo/arboard/tray: sempre via `run_on_main_thread` onde indicado
- HUD: `set_focusable(false)` (canBecomeKeyWindow=false) — NÃO roubar foco do app alvo
- Live: texto confirmado é FINAL — nunca re-decodificar áudio já commitado nem apagar o que foi digitado; worker amarrado à `hud_generation` da sessão e re-checando status/geração antes de digitar (texto de sessão morta nunca é digitado)
- Sem rede: tudo local (única exceção: download opt-in do modelo rápido via curl)
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
