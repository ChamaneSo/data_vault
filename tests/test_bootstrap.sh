#!/usr/bin/env bash
# Exercise credential generation and preservation without contacting a registry.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -f -- "$fixture/.env" "$fixture/.env.example" "$fixture/Cargo.lock" "$fixture/scripts/dev-init.sh"; rmdir "$fixture/scripts" "$fixture"' EXIT
mkdir "$fixture/scripts"
cp "$repo/scripts/dev-init.sh" "$fixture/scripts/"
sed 's/\r$//; s/$/\r/' "$repo/.env.example" > "$fixture/.env.example"
printf 'version = 4\n' > "$fixture/Cargo.lock"
bash "$fixture/scripts/dev-init.sh" >/dev/null
grep -Eq '^DATAVAULT_DATABASE_PASSWORD=[0-9a-f]{64}$' "$fixture/.env"
grep -Eq '^DATAVAULT_ADMIN_TOKEN=[0-9a-f]{64}$' "$fixture/.env"
grep -Eq '^DATAVAULT_MASTER_KEY=[A-Za-z0-9+/]{43}=$' "$fixture/.env"
first=$(sha256sum "$fixture/.env" "$fixture/Cargo.lock")
bash "$fixture/scripts/dev-init.sh" >/dev/null
second=$(sha256sum "$fixture/.env" "$fixture/Cargo.lock")
test "$first" = "$second"
echo 'PASS: generated credentials have the required shape; a second bootstrap preserves credentials and lockfile.'
