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
1. **Identidade de assinatura estável "Fala Dev"** — PENDENTE do usuário: Acesso às Chaves → Assistente de Certificados → Criar Certificado → nome `Fala Dev`, Raiz Autoassinada, tipo **Code Signing**. Motivo: assinatura ad-hoc muda por build → macOS esquece TCC (mic/Acessibilidade) a cada rebuild → prompts + falha de colagem. Depois de criada: `bash scripts/sign.sh` pós-build. (A importação via openssl+security CLI NÃO gera identidade utilizável no macOS 26 — testado.)
2. **DESIGN (em andamento)** — visual atual "não parece nativo" segundo o usuário. Spec Apple-native via agente de design com skills (ver brief abaixo). Usuário pode fornecer imagens de referência.
3. Fase 2: polimento IA (Ollama/cloud), streaming, ícone do app, assinatura Windows, auto-update.

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
