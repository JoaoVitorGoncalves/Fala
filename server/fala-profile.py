#!/usr/bin/env python3
"""fala-profile — serviço mínimo de personalização do Fala.

Um único JSON (system_prompt, vocab_prompt, corrections) que os dois
dispositivos puxam ao iniciar e enviam ao salvar. Sem banco, sem framework.

Rotas:
    GET  /health    → {"ok": true}
    GET  /profile   → JSON do perfil
    PUT  /profile   → grava o JSON (atômico: temp + rename)

Auth: Authorization: Bearer <FALA_TOKEN>. O acesso é pela interface Tailscale
(trafego WireGuard entra pelo túnel, não pelo gateway da VCN — nenhuma porta
precisa estar aberta na security list da Oracle).
"""
import json
import os
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PROFILE_PATH = os.environ.get("FALA_PROFILE_PATH", "/etc/fala/personalizacao.json")
TOKEN = os.environ.get("FALA_TOKEN", "")
PORT = int(os.environ.get("FALA_PORT", "8338"))


class Handler(BaseHTTPRequestHandler):
    def _authorized(self) -> bool:
        if not TOKEN:
            return True  # sem token configurado: rede Tailscale já é privada
        return self.headers.get("Authorization", "") == f"Bearer {TOKEN}"

    def _send(self, code: int, body: bytes) -> None:
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self._send(200, b'{"ok": true}')
            return
        if self.path != "/profile":
            self._send(404, b'{"error": "not found"}')
            return
        if not self._authorized():
            self._send(401, b'{"error": "unauthorized"}')
            return
        try:
            with open(PROFILE_PATH, "rb") as f:
                self._send(200, f.read())
        except FileNotFoundError:
            self._send(200, json.dumps({}).encode())  # perfil ainda vazio
        except OSError as e:
            self._send(500, json.dumps({"error": str(e)}).encode())

    def do_PUT(self):
        if self.path != "/profile":
            self._send(404, b'{"error": "not found"}')
            return
        if not self._authorized():
            self._send(401, b'{"error": "unauthorized"}')
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            json.loads(raw)  # valida antes de escrever qualquer coisa
            directory = os.path.dirname(PROFILE_PATH)
            os.makedirs(directory, exist_ok=True)
            fd, tmp = tempfile.mkstemp(dir=directory, suffix=".tmp")
            with os.fdopen(fd, "wb") as f:
                f.write(raw)
            os.replace(tmp, PROFILE_PATH)  # atômico — leitor nunca vê metade
            self._send(200, b'{"ok": true}')
        except (ValueError, json.JSONDecodeError) as e:
            self._send(400, json.dumps({"error": f"json invalido: {e}"}).encode())
        except OSError as e:
            self._send(500, json.dumps({"error": str(e)}).encode())

    def log_message(self, fmt, *args):
        print("[fala-profile]", self.address_string(), fmt % args, flush=True)


if __name__ == "__main__":
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    print(f"[fala-profile] escutando :{PORT} → {PROFILE_PATH}", flush=True)
    server.serve_forever()
