# 🎙️ Fala

Ditador local push-to-talk para macOS e Windows — estilo Wispr Flow, mas **100% local**: o áudio nunca sai da máquina.

Segure o atalho global → fale → solte → o resultado (transcrito pelo whisper.cpp com Metal/CPU) é guardado no Clipboard ou inserido no campo focado, conforme a configuração.

## Funcionalidades (v1)

- Atalho global push-to-talk (configurável, padrão `alt+space`)
- Whisper local (ggml), com aceleração Metal no macOS
- Entrega por Clipboard persistente (para colagem manual) ou inserção direta por digitação sintética
- **Transcrição em tempo real por commits**: cada pausa da fala fecha um trecho
  que é decodificado uma única vez (modelo rápido `small-q5_1`) e digitado como
  texto final — nada é apagado nem re-transcrito; ao soltar o atalho, só o
  trecho que ficou aberto recebe a revisão rápida do modelo grande. Ative em
  Configurações — o app oferece o download único do modelo rápido (~181 MB).
- Tray com estados: pronto / ● rec / … txt
- Configurações: atalho, idioma, microfone, modelo, limite de gravação, auto-start
- Onboarding de permissões (Acessibilidade no macOS)

## Polimento IA

Quando ativado, o Fala aguarda a versão limpa antes de entregar o resultado ao Clipboard ou ao campo focado. Se a IA estiver indisponível, a transcrição original é usada como fallback; sem IA, a entrega direta continua imediata.

Outras melhorias futuras incluem snippets por atalho falado, auto-update e assinatura de código no Windows.

## Desenvolvimento

```bash
pnpm install
pnpm tauri dev      # roda com recarga
pnpm tauri build    # .app/.dmg (macOS) ou NSIS/MSI (Windows)
```

Requisitos: Node + pnpm, Rust (rustup), cmake. No macOS basta o Xcode CLT.

### Modelo

O app detecta automaticamente um modelo existente em `~/.zcode/voice-poc/models/`.
Para baixar um novo (ggml):

```bash
mkdir -p ~/.fala/models && curl -L -o ~/.fala/models/ggml-large-v3-turbo-q5_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin
```

E aponte o caminho nas configurações do app.

### Windows

Build local na máquina Windows: `pnpm tauri build` (gera instalador NSIS).
O workflow `.github/workflows/release.yml` também gera os instaladores dos dois SOs via CI.

## Arquitetura

```
src-tauri/src/
├─ lib.rs       bootstrap, tray, máquina de estados push-to-talk, comandos Tauri
├─ settings.rs  persistência (JSON em app_config_dir)
├─ audio.rs     cpal: captura do mic → 16 kHz mono (reamostragem linear)
├─ stt.rs       whisper-rs: contexto persistente, Metal no macOS
├─ live.rs      transcrição em tempo real por commits (modelo rápido, trecho por pausa)
└─ inject.rs    arboard (clipboard) + enigo (Cmd/Ctrl+V) com fallback de digitação
```

## Permissões (macOS)

- **Microfone**: prompt do TCC na primeira gravação
- **Acessibilidade**: necessária para a colagem (Ajustes → Privacidade e Segurança → Acessibilidade)

Privacidade: o áudio e a transcrição permanecem locais por padrão. O download opcional do modelo rápido e o polimento IA usam rede somente quando configurados pelo usuário.
