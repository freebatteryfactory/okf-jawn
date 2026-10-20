#!/bin/sh
# Check the transferred Linux demo, without opening or changing its data store.
set -eu
demo_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
case "${1:-}" in
  '') full=1 ;;
  --runtime-only) full=0 ;;
  *) printf 'Usage: %s [--runtime-only]\n' "$0" >&2; exit 2 ;;
esac
if [ "$(uname -m)" != x86_64 ]; then
  printf 'This bundle requires 64-bit x86 Linux (x86_64).\n' >&2
  exit 1
fi
required=$(sed -n 's/^glibc_min=//p' "$demo_root/runtime-requirements.txt")
case "$required" in ''|*[!0-9.]*) printf 'Missing or invalid glibc requirement.\n' >&2; exit 1 ;; esac
current=$(getconf GNU_LIBC_VERSION | sed 's/^glibc //')
oldest=$(printf '%s\n%s\n' "$required" "$current" | sort -V | head -n 1)
if [ "$oldest" != "$required" ]; then
  printf 'This build needs glibc %s or newer; this machine has %s.\nUse a build for your distribution; do not replace system glibc.\n' "$required" "$current" >&2
  exit 1
fi
for program in okf-jawn-server okf-jawn okf-jawn-convert; do
  if [ ! -x "$demo_root/bin/$program" ]; then
    printf 'Missing executable: %s\n' "$program" >&2
    exit 1
  fi
  dependencies=$(ldd "$demo_root/bin/$program" 2>&1) || {
    printf '%s\n' "$dependencies" >&2
    exit 1
  }
  if printf '%s\n' "$dependencies" | grep -q 'not found'; then
    printf '%s\n' "$dependencies" >&2
    exit 1
  fi
done
"$demo_root/bin/okf-jawn-server" --help >/dev/null
"$demo_root/bin/okf-jawn" --help >/dev/null
if [ "$full" = 1 ]; then
  (cd "$demo_root" && sha256sum --check SHA256SUMS)
fi
printf 'Linux runtime checks passed (x86_64, glibc %s).\n' "$current"
