#!/usr/bin/env bash
# demo-window.sh — open a Postrinch window on a freshly seeded demo store,
# for the website's screenshots or for trying the window with no account.
#
#   tools/demo-window.sh [--dark] [--debug] [scratch folder]
#
# The window is cut off from this machine's own Postrinch:
#   * XDG_DATA_HOME  → <scratch>/data   the demo store (crates/postrinch-demo),
#                                        never ~/.local/share/postrinch
#   * XDG_CONFIG_HOME→ <scratch>/config only the demo's settings.json: no
#                                        Google sign-in client, so sync is off
#                                        and nothing reaches Google
#   * HOME           → <scratch>/home   so Settings › Agents reads and writes
#                                        no real agent configuration
#   * POSTRINCH_APP_ADDR → 127.0.0.1:7466, off the installed window's port
#
# With sync off the window says so in the sidebar's Sync section ("Sync is
# off: Google sign-in is not set up on this computer."), and an invitation
# card says it cannot reach Google Calendar to answer. That is the truth
# about this window, and the screenshots show it as it is.
#
# --dark   start in the dark appearance (default light).
# --debug  build with rinch's debug server and start it (RINCH_DEBUG=1), and
#          copy the window's discovery file into ~/.rinch/debug so the rinch
#          tools can connect; the copy is removed when the window closes.
#
# The scratch folder (default: a new one under ${TMPDIR:-/tmp}) is emptied
# and seeded again on every run, so times are relative to now.
set -euo pipefail

appearance=Light
debug=0
scratch=""
for arg in "$@"; do
  case "$arg" in
    --dark) appearance=Dark ;;
    --debug) debug=1 ;;
    -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
    -*) echo "demo-window: unknown option $arg" >&2; exit 2 ;;
    *) scratch="$arg" ;;
  esac
done

repo=$(cd "$(dirname "$0")/.." && pwd -P)
if [[ -z "$scratch" ]]; then
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/postrinch-demo.XXXXXX")
fi
mkdir -p "$scratch"
scratch=$(cd "$scratch" && pwd -P)

# Never let the scratch folder be (or hold) this machine's own data.
for own in "$HOME" "${XDG_DATA_HOME:-$HOME/.local/share}" "${XDG_CONFIG_HOME:-$HOME/.config}"; do
  own=$(cd "$own" 2>/dev/null && pwd -P || echo "$own")
  if [[ "$scratch" == "$own" ]]; then
    echo "demo-window: $scratch is your own folder; give a scratch folder" >&2
    exit 2
  fi
done

features=()
[[ $debug == 1 ]] && features=(--features debug)
cargo build --release --manifest-path "$repo/Cargo.toml" -p postrinch-demo
cargo build --release --manifest-path "$repo/Cargo.toml" -p postrinch "${features[@]}"
target=$(cargo metadata --manifest-path "$repo/Cargo.toml" --format-version 1 --no-deps |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')

rm -rf -- "$scratch/data" "$scratch/config" "$scratch/home"
mkdir -p "$scratch/data" "$scratch/config/postrinch" "$scratch/home"
"$target/release/postrinch-demo" "$scratch/data/postrinch"
cat > "$scratch/config/postrinch/settings.json" <<EOF
{
  "appearance": "$appearance",
  "calendar_span": "Week",
  "view_tree": {
    "inbox:sam@example.com": true,
    "inbox:sam@lindenworks.example": true
  }
}
EOF

env_vars=(
  XDG_DATA_HOME="$scratch/data"
  XDG_CONFIG_HOME="$scratch/config"
  HOME="$scratch/home"
  POSTRINCH_APP_ADDR=127.0.0.1:7466
)
[[ $debug == 1 ]] && env_vars+=(RINCH_DEBUG=1)

env "${env_vars[@]}" "$target/release/postrinch" > "$scratch/window.log" 2>&1 &
pid=$!
echo "Demo window: pid $pid, scratch $scratch (log: $scratch/window.log)"

copied=""
cleanup() {
  [[ -n "$copied" ]] && rm -f -- "$copied"
  return 0
}
trap cleanup EXIT
trap 'kill "$pid" 2>/dev/null || true' INT TERM

if [[ $debug == 1 ]]; then
  found="$scratch/home/.rinch/debug/$pid.json"
  for _ in $(seq 1 100); do
    [[ -f "$found" ]] && break
    sleep 0.1
  done
  if [[ -f "$found" ]]; then
    mkdir -p "$HOME/.rinch/debug"
    copied="$HOME/.rinch/debug/$pid.json"
    cp -- "$found" "$copied"
    echo "rinch debug server ready: connect to pid $pid"
  else
    echo "demo-window: the window wrote no debug discovery file (see $scratch/window.log)" >&2
  fi
fi
wait "$pid" || true
