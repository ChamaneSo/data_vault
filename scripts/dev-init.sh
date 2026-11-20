#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
umask 077
if [[ ! -f .env ]]; then
  # Publish only the complete file. Concurrent initializers keep the first winner.
  tmp=$(mktemp ./.env.XXXXXX)
  trap 'rm -f -- "$tmp"' EXIT
  while IFS= read -r line || [[ -n "$line" ]]; do
    line=${line%$'\r'}
    case "$line" in
      DATAVAULT_DATABASE_PASSWORD=|DATAVAULT_ADMIN_TOKEN=)
        value=$(openssl rand -hex 32)
        printf '%s%s\n' "$line" "$value" ;;
      DATAVAULT_MASTER_KEY=)
        value=$(openssl rand -base64 32)
        printf '%s%s\n' "$line" "$value" ;;
      *) printf '%s\n' "$line" ;;
    esac
  done < .env.example > "$tmp"
  # A root-run container must leave the private file readable by the checkout owner.
  if [[ $(id -u) == 0 ]]; then chown --reference=. "$tmp"; fi
  if ! ln "$tmp" .env; then test -s .env; fi
  echo 'Development configuration created.'
fi
if [[ ! -f Cargo.lock ]]; then
  cargo generate-lockfile
  chmod 644 Cargo.lock
  if [[ $(id -u) == 0 ]]; then chown --reference=. Cargo.lock; fi
fi
echo 'DataVault development configuration and Cargo.lock are ready.'
