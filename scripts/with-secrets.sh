#!/usr/bin/env bash
# Runs a command with the signing secrets in its environment: a mounted .env
# takes precedence, otherwise .env.1password is resolved live via `op run`.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f .env ]; then
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in ''|\#*) continue;; esac
        case "$line" in *=*) ;; *) continue;; esac
        key=${line%%=*}
        value=${line#*=}
        case "$key" in [A-Za-z_]*) ;; *) continue;; esac
        case "$value" in
            \"*\") value=${value#\"}; value=${value%\"};;
            \'*\') value=${value#\'}; value=${value%\'};;
        esac
        export "$key=$value"
    done < ./.env
    exec "$@"
fi

[ -f .env.1password ] || { echo "error: no secrets source found — create .env (see .env.example) or restore .env.1password"; exit 1; }
command -v op >/dev/null 2>&1 || { echo "error: 1Password CLI (op) not found."; exit 1; }
exec op run --env-file=.env.1password -- "$@"
