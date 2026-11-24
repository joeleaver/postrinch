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
#   * XDG_CACHE_HOME, XDG_STATE_HOME → <scratch>/cache, <scratch>/state, so
#                                        opened attachments and logs stay there
#   * CODEX_HOME unset, so Codex is looked for in the scratch home only
#   * POSTRINCH_APP_ADDR → 127.0.0.1:7466, off the installed window's port
#
# In that home the demo person has Claude and Cursor (empty .claude/,
# Claude/ and .cursor/ folders), so Settings › Agents finds them and
# connecting one writes its settings there.
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
# The scratch folder (default: a new one under ${TMPDIR:-/tmp}) must be new,
# empty, or one this script made before; it is emptied and seeded again on
# every run, so times are relative to now.
set -euo pipefail

appearance=Light
debug=0
scratch=""
for arg in "$@"; do
  case "$arg" in
    --dark) appearance=Dark ;;
    --debug) debug=1 ;;
    -h|--help) sed -n '2,36p' "$0"; exit 0 ;;
    -*) echo "demo-window: unknown option $arg" >&2; exit 2 ;;
    *) scratch="$arg" ;;
  esac
done

repo=$(cd "$(dirname "$0")/.." && pwd -P)
if [[ -z "$scratch" ]]; then
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/postrinch-demo.XXXXXX")
fi

# The folder is emptied of data/, config/, home/, cache/ and state/ below,
# so only take one that is new, empty, or marked as this script's by an
# earlier run: never a folder that holds anything else.
marker=.postrinch-demo-scratch
if [[ -e "$scratch" && ! -d "$scratch" ]]; then
  echo "demo-window: $scratch is not a folder" >&2
  exit 2
fi
if [[ -d "$scratch" && ! -e "$scratch/$marker" ]]; then
  # A folder that cannot be listed is refused too: it may hold anything.
  if ! listing=$(ls -A -- "$scratch") || [[ -n "$listing" ]]; then
    echo "demo-window: $scratch is not empty and is not a demo scratch folder; give a new or empty one" >&2
    exit 2
  fi
fi
mkdir -p "$scratch"
scratch=$(cd "$scratch" && pwd -P)
touch -- "$scratch/$marker"

features=()
[[ $debug == 1 ]] && features=(--features debug)
cargo build --release --manifest-path "$repo/Cargo.toml" -p postrinch-demo
cargo build --release --manifest-path "$repo/Cargo.toml" -p postrinch "${features[@]}"
target=$(cargo metadata --manifest-path "$repo/Cargo.toml" --format-version 1 --no-deps |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')

rm -rf -- "$scratch/data" "$scratch/config" "$scratch/home" "$scratch/cache" "$scratch/state"
mkdir -p "$scratch/data" "$scratch/config/postrinch" "$scratch/home" "$scratch/cache" "$scratch/state"
# The demo person has Claude (Claude Code and Claude Desktop) and Cursor:
# Settings › Agents finds an app by its folder, so these empty folders are
# all it takes. Connecting one there writes only inside this scratch home.
mkdir -p "$scratch/home/.claude" "$scratch/config/Claude" "$scratch/home/.cursor"
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
  XDG_CACHE_HOME="$scratch/cache"
  XDG_STATE_HOME="$scratch/state"
  POSTRINCH_APP_ADDR=127.0.0.1:7466
)
[[ $debug == 1 ]] && env_vars+=(RINCH_DEBUG=1)

# CODEX_HOME would point Settings › Agents at this machine's own Codex.
env -u CODEX_HOME "${env_vars[@]}" "$target/release/postrinch" > "$scratch/window.log" 2>&1 &
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
