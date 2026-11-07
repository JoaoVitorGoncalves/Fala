#!/usr/bin/env bash
# Fala — instala Ollama + serviço de personalização no servidor Oracle (Ubuntu ARM).
# Rodar como root DEPOIS do Tailscale estar instalado e autenticado:
#   sudo bash setup.sh
# Idempotente: pode rodar de novo sem quebrar nada.
set -euo pipefail

FALA_DIR=/opt/fala
PROFILE_JSON=/etc/fala/personalizacao.json
MODEL="${FALA_MODEL:-llama3.2:3b}"

cd "$(dirname "$0")"

echo "==> Pré-requisito: Tailscale"
if ! command -v tailscale >/dev/null 2>&1; then
  echo "    Instale e autentique antes de rodar este script:"
  echo "      curl -fsSL https://tailscale.com/install.sh | sh"
  echo "      sudo tailscale up"
  exit 1
fi
tailscale status >/dev/null 2>&1 || { echo "    Tailscale não autenticado: rode 'sudo tailscale up'"; exit 1; }

echo "==> Usuário e diretórios do serviço"
id -u fala >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin fala
mkdir -p "$FALA_DIR" /etc/fala
cp fala-profile.py "$FALA_DIR/"
[ -f "$PROFILE_JSON" ] || cp personalizacao.json "$PROFILE_JSON"

echo "==> Token do serviço"
# openssl rand: sem pipe sobre stream infinita (tr|head morre de SIGPIPE com pipefail)
if [ ! -s /etc/fala/token ]; then
  openssl rand -hex 16 > /etc/fala/token
  chmod 600 /etc/fala/token
fi
# o systemd lê FALA_TOKEN deste arquivo (EnvironmentFile no unit)
grep -q FALA_TOKEN /etc/fala/token.env 2>/dev/null || echo "FALA_TOKEN=$(cat /etc/fala/token)" > /etc/fala/token.env
chmod 640 /etc/fala/token.env
chgrp fala /etc/fala/token.env /etc/fala/token /etc/fala/personalizacao.json
chown fala "$PROFILE_JSON" 2>/dev/null || true

echo "==> Ollama"
if ! command -v ollama >/dev/null 2>&1; then
  curl -fsSL https://ollama.com/install.sh | sh
fi
# O padrão do Ollama é 127.0.0.1 — o Tailscale não alcança; escuta em 0.0.0.0
# (a security list da Oracle continua fechada: só a rede privada Tailscale chega).
mkdir -p /etc/systemd/system/ollama.service.d
cat > /etc/systemd/system/ollama.service.d/fala.conf <<'EOF'
[Service]
Environment="OLLAMA_HOST=0.0.0.0"
EOF
systemctl daemon-reload
systemctl enable --now ollama
systemctl restart ollama
# o bind da porta leva ~1-2s — sem este wait, o pull logo abaixo dá "could not connect"
for _ in $(seq 1 30); do
  curl -sf http://127.0.0.1:11434/ >/dev/null 2>&1 && break
  sleep 1
done

echo "==> Baixando modelo $MODEL (CPU do servidor; pode demorar alguns minutos)"
ollama pull "$MODEL"

echo "==> Serviço fala-profile"
cp fala-profile.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now fala-profile

TS_IP=$(tailscale ip -4 2>/dev/null | head -1)

echo
echo "======================================================================"
echo " Instalação concluída."
echo
echo " No app Fala (Mac e Windows), em Configurações → 'IA do servidor', use:"
echo
echo "   Endpoint:       http://$TS_IP:11434"
echo "   Personalização: http://$TS_IP:8338"
echo "   Token:          $(cat /etc/fala/token)"
echo "   Modelo:         $MODEL"
echo
echo " Teste rápido pelo próprio servidor:"
echo "   curl http://localhost:8338/health"
echo "   curl -s http://localhost:11434/api/tags | head"
echo "======================================================================"
