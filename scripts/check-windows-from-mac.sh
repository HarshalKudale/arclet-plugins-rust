#!/bin/sh
# Type-check every plugin for Windows from a Mac or Linux (no linking, no
# Windows SDK). C sources (ring, bundled SQLite) are stubbed out:
# `cargo check` never links, so only the Rust code is actually verified.
# Needs: rustup target add x86_64-pc-windows-msvc
# Another OS: TARGET=aarch64-apple-darwin ./scripts/check-windows-from-mac.sh
set -e
target=${TARGET:-x86_64-pc-windows-msvc}
target_env=$(echo "$target" | tr '-' '_')
stub_dir=$(mktemp -d)
trap 'rm -rf "$stub_dir"' EXIT
cat > "$stub_dir/stub" <<'STUB'
#!/bin/sh
# Create whatever output files were requested; compile nothing.
prev=""
for a in "$@"; do
  case "$a" in
    -out:*) touch "${a#-out:}" ;;
    -Fo*) [ -n "${a#-Fo}" ] && touch "${a#-Fo}" ;;
    *.a|*.lib) touch "$a" ;;
  esac
  case "$prev" in -o|/fo|/FO) touch "$a" ;; esac
  prev="$a"
done
exit 0
STUB
chmod +x "$stub_dir/stub"
for tool in llvm-rc llvm-lib llvm-ar; do ln -s "$stub_dir/stub" "$stub_dir/$tool"; done
cd "$(dirname "$0")/.."
env PATH="$stub_dir:$PATH" \
  "CC_$target_env=$stub_dir/stub" \
  "AR_$target_env=$stub_dir/stub" \
  cargo check --workspace --all-targets --target "$target" "$@"
