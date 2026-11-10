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
- **IA do servidor (branch `feature/ia-servidor`, PR #7 → dev)**: polimento LLM via Ollama no Oracle (Tailscale) + perfil de personalização ÚNICO entre dispositivos. `llm.rs` (ureq, sem TLS — túnel; `polish`/`check`/`get_profile`/`put_profile`); settings `ai: Option<AiSettings>`; comandos `test_ai`/`sync_profile` (eventos `ai-test-result`/`profile-synced`); write-through no `set_settings` (`profile_changed`); sync no boot. Guardas: envelope de DADO no prompt + `llm::similarity` (Levenshtein <0.6 = resposta/recusa → descarta) + tamanho/markdown. Servidor REAL rodando: `fala-servidor` (Tailscale 100.86.38.3) com Ollama `llama3.2:3b` (`OLLAMA_HOST=0.0.0.0`, `KEEP_ALIVE=30m`) + `fala-profile.py` (:8338, Bearer token em `/etc/fala/token`); pacote em `server/` (setup.sh idempotente + README com resgate de SSH pela console Oracle). Polimento medido: ~5-9s morno no 3b (2 vCPU/11GB).
- **P2 (branch `feature/melhorias-ia`) — app diário**:
  - **Colar rápido (fase 0)**: a injeção NUNCA espera a IA — cauda injeta após correções locais; worker de polish async (fila + colapso de backlog ≥3 em `AppState.polish_queue`) emite `clean-version` e alimenta o painel. `polish_before_paste` (default false) restaura o comportamento clássico. Trechos ao vivo também enfileiram. Mic travado (silêncio digital ~100% após fala) avisa no HUD em vez de sucesso silencioso.
  - **Painel "Versão limpa" (fase 1)**: 3ª janela `panel` (hudWindow radius 20, alwaysOnTop, skipTaskbar) — **precisa estar em `capabilities/default.json`** (ACL). `AppState.panel` (Vec<PanelSegment> raw/clean), eventos `panel-state` (broadcast full-state), worker colapsa lote [first..=last] num segmento limpo. Comandos `panel_state/panel_copy/panel_clear`; `inject::copy_to_clipboard` (sem restaurar). Tray "Versão limpa" toggle (`toggle_panel`, borda direita). `TranscriptPanel.tsx` + `panel.css` (fade in, shimmer "⋯" no pendente, realce verde no swap cru→limpo, auto-scroll).
  - **Comandos de voz (fase 2)**: frase exata normalizada (lowercase/sem acentos/pontuação) → `VoiceCommand`: "novo parágrafo" (2× Enter), "apagar tudo" (backspace total), "apagar último" (topo da pilha), "copiar versão limpa". `AppState.injected` (chars colados por trecho) é a base dos backspaces. `inject::backspace_on_main/enter_on_main` (Key::Backspace/**Return** — enigo 0.6 não tem `Enter`). Detecção nos commits (live.rs) e na cauda (lib.rs); comando nunca vira texto nem entra no painel. Feedback: chip transitório `hud-chip` no HUD (não troca estado — gravação segue intacta).
  - **Modo reunião (fase 3)**: `meeting_mode` — commits ao vivo não digitam (só painel + contexto do prompt); cauda final também não injeta; tray "capturando sem colar" + chip ao iniciar.
  - **Conflito HFP**: `audio.rs` consulta CoreAudio direto (transporte 'blue'/'blle'/'bltn' — cpal não expõe); conflito = entrada BT com saída BT → aviso na seção Microfone + botão "Trocar microfone" (prefere embutido). Comando `audio_conflict`. Sonda: `cargo test bt_conflict -- --nocapture`.
- Tray 3 estados (● rec / … txt); HUD flutuante não-focável com waveform ao vivo, spinner, ✓ animado
- Diagnóstico: mapa de níveis (RMS 2s) + % silêncio digital + dispositivo por gravação; eventos do atalho no log; panics em `~/.fala/panic.log`

## Arquitetura (src-tauri/src/)
- `lib.rs` — bootstrap, tray, máquina de estados (Idle/Recording/Transcribing), watcher (níveis + auto-stops + Esc), comandos Tauri, `apply_corrections`
- `audio.rs` — cpal capture → 16 kHz mono (downmix + resample linear), `VadState` (rms/speech_seen/last_voice), `RecorderHandle` (snapshot não-destrutivo p/ o live)
- `stt.rs` — whisper-rs 0.16 (contexto persistente, Metal), trim de silêncio, clean_text, `base_params` compartilhado
- `live.rs` — worker de commits ao vivo (ctx pequeno dedicado, `WhisperState` reusado, digitação via `inject`)
- `llm.rs` — cliente da IA remota: `polish` (POST /api/chat, stream off), `check` (/api/tags), `get_profile`/`put_profile` (serviço fala-profile, Bearer)
- `inject.rs` — paste/insert; `on_main()` para enigo (HIToolbox exige main thread), `type_text_on_main`
- `settings.rs` — JSON em app_config_dir, `#[serde(default)]`, `live_transcribe`/`live_model_path` + `live_model_path_or_default`

## Pendências / issues abertas
1. **Identidade estável "Fala Dev" — RESOLVIDA**: cert autoassinado criado via Certificate Assistant (GUI) no login keychain; `scripts/sign.sh` re-assina o bundle pós-build (assinar por NOME — `find-identity -p codesigning` não lista autoassinados, mas o codesign usa). PRIMEIRO uso: usuário concedeu mic/Acessibilidade uma última vez; agora persiste entre rebuilds. ⚠️ `pnpm tauri build` assina ad-hoc por cima → SEMPRE rodar `scripts/sign.sh` depois.
2. **Crash device_query resolvido**: device_query faz `panic!` sem Acessibilidade (macos/mod.rs:122); monitor de teclado só é criado com permissão confirmada + `catch_unwind` em cada `get_keys()`.
3. **DMG**: `hdiutil create failed - Resource busy` no bundling (volume antigo montado); o .app assinado em `target/release/bundle/macos/` segue válido — DMG é só distribuição. Notarização (APPLE_ID…) e ícone do app seguem pendentes (alvo: distribuição futura — usuário decidiu que o app é para uso próprio por enquanto).
4. **PR #7** (`feature/ia-servidor` → `dev`) aberta aguardando merge; `feature/melhorias-ia` empilhada em cima (P2 completo: colar rápido, painel versão limpa, comandos de voz, modo reunião, conflito HFP). Falta validação do usuário no app real + push/PR.
5. **Mic travado (BT stall)**: intermitente, se auto-cura; já instrumentado (dispositivo + % silêncio digital + aviso no HUD). Se recorrente, considerar auto-restart do stream cpal.
6. Windows: configurar IA no app (5 min na UI) e validar painel/comandos lá; assinatura Windows e auto-update pendentes.

## Constraints técnicos que NÃO podem regredir
- enigo/arboard/tray: sempre via `run_on_main_thread` onde indicado
- HUD: `set_focusable(false)` (canBecomeKeyWindow=false) — NÃO roubar foco do app alvo
- Live: texto confirmado é FINAL — nunca re-decodificar áudio já commitado nem apagar o que foi digitado; worker amarrado à `hud_generation` da sessão e re-checando status/geração antes de digitar (texto de sessão morta nunca é digitado)
- **IA nunca bloqueia a injeção**: padrão é colar rápido (correções locais); o polimento é async (`polish_before_paste` é opt-in para voltar a esperar). Comandos de voz nunca viram texto nem entram no painel
- **Janela nova → capabilities**: adicionar o label em `capabilities/default.json` ou invoke/listen falham por ACL
- Sem rede por padrão: tudo local (exceções opt-in: download do modelo rápido via curl; IA do servidor, que degrada para texto sem polir se o servidor estiver fora — trechos ao vivo NUNCA tocam a IA, e a transcrição nunca falha por rede)
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
