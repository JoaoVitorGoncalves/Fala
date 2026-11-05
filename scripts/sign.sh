#!/bin/bash
# Re-assina o Fala.app com a identidade estável "Fala Dev" + entitlement de microfone.
# Roda DEPOIS de cada `pnpm tauri build`. Uso: bash scripts/sign.sh
set -e
APP="$HOME/Projects/fala/src-tauri/target/release/bundle/macos/Fala.app"
ENT="$HOME/Projects/fala/scripts/fala-entitlements.plist"
CN="Fala Dev"

# find-identity -p codesigning não lista autoassinados; o certificado existir no
# keychain basta — o codesign resolve a identidade pelo nome.
if ! security find-certificate -c "$CN" ~/Library/Keychains/login.keychain-db >/dev/null 2>&1; then
  echo "certificado '$CN' não encontrado — rode: bash scripts/dev-identity.sh (passos GUI)"
  exit 1
fi

codesign --force --options runtime --entitlements "$ENT" --sign "$CN" "$APP"
codesign --verify --verbose=1 "$APP" 2>&1 | tail -1
echo "assinado com identidade estável '$CN'"
