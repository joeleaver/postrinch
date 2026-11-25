#!/usr/bin/env bash
# Runs the IMAP/SMTP smoke test against a real server, GreenMail
# (docs/imap.md, "Testing"): downloads GreenMail's standalone jar from Maven
# Central once, checks its SHA-256 against the one pinned here, and runs the
# `greenmail` test of postrinch-sync with POSTRINCH_GREENMAIL_JAR naming it.
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

# To move to a newer GreenMail: change both, taking the SHA-256 from
# Maven Central's `.sha256` beside the jar and checking it against the
# downloaded file.
version=2.1.15
sha256=91d58798698cac357f015365240383b936ced2f2a1a9f7db37d9e3e1a31391b1

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
    curl --fail --silent --show-error --location --output "$partial" "$url"
    if ! checked "$partial"; then
      echo "the downloaded $jar_name does not have the SHA-256 pinned in $0" >&2
      exit 1
    fi
    mv "$partial" "$jar"
  fi
  export POSTRINCH_GREENMAIL_JAR="$jar"
fi

cd "$repo_root"
"${CARGO:-cargo}" test --release -p postrinch-sync --test greenmail -- --include-ignored "$@"
