#!/bin/bash
# Re-assina o Fala.app com a identidade estável "Fala Dev" + entitlement de microfone.
# Roda DEPOIS de cada `pnpm tauri build`. Uso: bash scripts/sign.sh
set -e
APP="$HOME/Projects/fala/src-tauri/target/release/bundle/macos/Fala.app"
ENT="$HOME/Projects/fala/scripts/fala-entitlements.plist"

IDENTITY=$(security find-identity -p codesigning -v 2>/dev/null | grep "Fala Dev" | awk '{print $2}' | head -1)
if [ -z "$IDENTITY" ]; then
  echo "identidade 'Fala Dev' não encontrada — rode: bash scripts/dev-identity.sh (passos GUI)"
  exit 1
fi

codesign --force --options runtime --entitlements "$ENT" --sign "$IDENTITY" "$APP"
codesign --verify --verbose=1 "$APP" 2>&1 | tail -1
echo "assinado com identidade estável"
