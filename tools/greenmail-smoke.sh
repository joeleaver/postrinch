#!/usr/bin/env bash
# Runs the IMAP/SMTP smoke test against a real server, GreenMail
# (docs/imap.md, "Testing"): downloads GreenMail's standalone jar from Maven
# Central once, checks its SHA-256 against the one pinned in
# tools/greenmail.pin, and runs the `greenmail` test of postrinch-sync with
# POSTRINCH_GREENMAIL_JAR naming it.
# The test starts GreenMail on loopback ports, with a certificate made for
# the run, and kills it when done. CI runs this script.
#
# Needs a JDK 17 or later (`java` and `keytool`, from $JAVA_HOME/bin when it
# is set) and curl.
#
# Usage: tools/greenmail-smoke.sh [extra arguments for the test binary]
#   CARGO=cargo-alt tools/greenmail-smoke.sh   (build in the second dir)
#   POSTRINCH_GREENMAIL_JAR=/path/to.jar tools/greenmail-smoke.sh
#                                               (use a jar you have)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The pinned release: `version` and its `sha256`.
# shellcheck source=greenmail.pin
. "$repo_root/tools/greenmail.pin"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/postrinch/greenmail"
jar_name="greenmail-standalone-$version.jar"
url="https://repo1.maven.org/maven2/com/icegreen/greenmail-standalone/$version/$jar_name"

checked() {
  echo "$sha256  $1" | sha256sum --check --status
}

if [ -z "${POSTRINCH_GREENMAIL_JAR:-}" ]; then
  jar="$cache/$jar_name"
  if [ ! -f "$jar" ] || ! checked "$jar"; then
    mkdir -p "$cache"
    partial="$(mktemp "$cache/.download.XXXXXX")"
    trap 'rm -f "$partial"' EXIT
    echo "Downloading GreenMail $version from Maven Central" >&2
    # Maven Central can answer 429 (too many requests) to shared CI
    # runners: curl retries that and other passing failures, after the
    # Retry-After the server gives.
    curl --proto =https --proto-redir =https --fail --silent --show-error --location \
      --retry 10 --retry-max-time 300 --output "$partial" "$url"
    if ! checked "$partial"; then
      echo "the downloaded $jar_name does not have the SHA-256 pinned in tools/greenmail.pin" >&2
      exit 1
    fi
    mv "$partial" "$jar"
  fi
  export POSTRINCH_GREENMAIL_JAR="$jar"
fi

cd "$repo_root"
# A filter that matches nothing runs no test and passes; the smoke test
# must have run, so the output is read for how many passed.
log="$(mktemp)"
trap 'rm -f "$log" "${partial:-}"' EXIT
"${CARGO:-cargo}" test --release -p postrinch-sync --test greenmail -- --include-ignored "$@" \
  | tee "$log"
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no GreenMail test ran: check the filter passed to $0" >&2
  exit 1
fi
