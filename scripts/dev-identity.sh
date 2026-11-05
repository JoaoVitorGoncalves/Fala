#!/bin/bash
# Identidade de assinatura estável "Fala Dev" — evita que o macOS esqueça as
# permissões (mic/Acessibilidade) a cada rebuild.
#
# A importação via CLI (openssl + security import) não gera identidade utilizável
# no macOS 26 — use o Certificate Assistant (uma vez só):
#
#   1. Abra "Acesso às Chaves" (Keychain Access)
#   2. Menu: Acesso às Chaves → Assistente de Certificados → Criar um Certificado
#   3. Nome: Fala Dev
#      Identidade: Raiz Autoassinada (Self-Signed Root)
#      Tipo: Assinatura de Código (Code Signing)
#   4. Criar
#
# Depois confira: security find-identity -p codesigning -v
set -e
CN="Fala Dev"
if security find-identity -p codesigning -v 2>/dev/null | grep -q "$CN"; then
  echo "OK: identidade '$CN' existe"
  security find-identity -p codesigning -v | grep "$CN"
  exit 0
fi
echo "identidade '$CN' não encontrada — siga os passos do Certificate Assistant acima"
exit 1
