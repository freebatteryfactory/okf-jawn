#!/bin/sh
# Local-only demo entry point: the unchanged server, or its credential-scoped MCP relay.
set -eu
umask 077
demo_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
"$demo_root/check.sh" --runtime-only >/dev/null
export OKF_AUTH_MODE=local
export OKF_BIND=127.0.0.1:7711
export OKF_PUBLIC_ORIGIN=http://127.0.0.1:7711
export OKF_SANDBOX_BIND=127.0.0.1:7712
export OKF_SANDBOX_ORIGIN=http://127.0.0.1:7712
export OKF_UI_DIR="$demo_root/ui"
export OKF_MCP_APP="$demo_root/mcp-apps/app.html"
export OKF_CONVERTER="$demo_root/bin/okf-jawn-convert"
export DOCLING_RS_MODELS_DIR="$demo_root/models"
export OKF_DATA_DIR="${OKF_DEMO_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/okf-jawn-demo}"
case "$OKF_DATA_DIR" in
  /*) ;;
  *) printf 'OKF_DEMO_DATA_DIR must be an absolute path.\n' >&2; exit 2 ;;
esac
# These hosted/override settings must not change the local demo's host checks or asset paths.
unset OKF_ALLOWED_HOSTS OKF_TRUSTED_PROXIES OKF_CONVERTER_MEMORY_LIMIT_BYTES
case "${1:-}" in
  '') exec "$demo_root/bin/okf-jawn-server" ;;
  --mcp)
    shift
    exec "$demo_root/bin/okf-jawn" --server "$OKF_PUBLIC_ORIGIN" mcp --stdio "$@"
    ;;
  *) printf 'Usage: %s [--mcp --token-file /absolute/path/to/connector]\n' "$0" >&2; exit 2 ;;
esac
