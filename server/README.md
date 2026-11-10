# IA do Fala no servidor Oracle

Um só cérebro para todos os dispositivos: o whisper (o que precisa de GPU)
continua local no Mac/Windows; o **polimento LLM** e o **perfil de
personalização** (vocabulário, correções, prompt de sistema) ficam no
servidor, acessados via **Tailscale**.

```
Fala (Mac/Win)                      Oracle
─────────────────────               ─────────────────────────
mic → VAD → whisper local (GPU)     Ollama :11434        ← polimento
        ↓ correções locais          fala-profile :8338   ← perfil compartilhado
   revisão final ── HTTP ──────→    /etc/fala/personalizacao.json
        ↓
   inject
```

## 1. Recuperar acesso ao servidor (chave SSH perdida)

Pela console da Oracle Cloud (login e MFA bastam, sem SSH):

1. **Console → Compute → Instâncias → sua instância.**
2. Caminho mais fácil — *Executar comando* (Oracle Cloud Agent):
   - Aba **Oracle Cloud Agent**, confirme que *Run Command* está `RUNNING`.
   - **Commands / Run command** → criar comando com `bash`, conteúdo:
     ```bash
     mkdir -p ~/.ssh && echo 'ssh-ed25519 AAAA...SUA_CHAVE' >> ~/.ssh/authorized_keys && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys
     ```
     (gera a chave nova localmente com `ssh-keygen -t ed25519` e cola a pública)
   - Depois: `ssh ubuntu@<IP_DA_INSTANCIA>`
3. Alternativa: **console serial** (Instância → Console connection → Cloud Shell
   connection) e recriar a entrada em `authorized_keys` por lá.

## 2. Tailscale (nos 3 dispositivos)

Nada é exposto na internet: nenhuma porta abre na security list — o tráfego
entra pelo túnel WireGuard.

**No servidor (Ubuntu):**
```bash
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up          # abre um link de login no terminal
```

**No Mac:** App Store (`Tailscale`) ou `brew install --cask tailscale` → logar
na mesma conta.

**No Windows:** [tailscale.com/download](https://tailscale.com/download) →
logar na mesma conta.

## 3. Instalar a IA no servidor

Copie a pasta `server/` deste repo para a máquina (ou clone o repo) e rode:

```bash
sudo bash server/setup.sh
```

O script é idempotente e faz: usuário/diretórios, token aleatório, Ollama
com `OLLAMA_HOST=0.0.0.0` (o default `127.0.0.1` não é alcançável via
Tailscale), download do modelo (CPU — conta com os 24 GB de RAM do A1),
serviço `fala-profile` no systemd. Ao final imprime **endpoint, URL de
personalização, token e modelo** para colar no app.

Trocar de modelo depois (mais rápido, p/ testes): `FALA_MODEL=llama3.2:3b sudo bash server/setup.sh`
e atualize o campo "Modelo" no app.

## 4. Configurar o app (Mac e Windows)

Configurações → **IA do servidor** → ligar:

- **Ollama**: `http://<nome-do-servidor>:11434` (nome Tailscale ou o IP `100.x.y.z`)
- **Personalização**: `http://<nome-do-servidor>:8338`
- **Token**: o impresso pelo `setup.sh` (`cat /etc/fala/token` no servidor)
- **Modelo**: `llama3.2:3b`

Clique **Testar conexão** (deve listar os modelos) e **Sincronizar agora**.

## 5. Como funciona a sincronização

- **No boot** o app puxa o perfil do servidor → o servidor vence.
- **Ao salvar** (vocabulário, correções, prompt de sistema), o app envia ao
  servidor → o app vence, e o outro dispositivo recebe no próximo boot/sync.
- Editar uma vez vale para os dois computadores.

## Regras de segurança

- Security list da Oracle **continua fechada** — só 22 (SSH). Tailscale não
  usa o gateway da VCN.
- O `fala-profile` exige `Authorization: Bearer <token>`; o token vive em
  `/etc/fala/token` (chmod 600).
- O Ollama fica acessível apenas dentro da rede privada Tailscale.

## Desligar / manutenção

```bash
sudo systemctl restart ollama fala-profile     # reiniciar
sudo journalctl -u fala-profile -f             # logs do perfil
sudo journalctl -u ollama -f                   # logs do Ollama
tail -f /etc/fala/personalizacao.json          # perfil atual
```

No app: desligar o toggle "IA do servidor" volta ao comportamento 100% local
(o ditado nunca depende da rede — se o servidor cair, o texto entra sem
polir).
