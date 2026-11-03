#!/usr/bin/env bash
# Packages the three release-built programs into the tar.gz the Release
# workflow attaches to a GitHub release (DESIGN.md, "Installing and
# updating"), so the same step can be run and inspected here.
#
# Usage: tools/package.sh [version] [bin-dir] [out-dir]
#
#   version  Defaults to the workspace version in Cargo.toml.
#   bin-dir  Defaults to target/release. Not built here: run
#            `cargo build --release -p postrinch -p postrinch-mcp -p postrinch-cli`
#            first, WITHOUT the `debug` feature.
#   out-dir  Defaults to dist/.
#
# Writes <out-dir>/postrinch-<version>-linux-<arch>.tar.gz holding
# postrinch, postrinch-mcp and postrinch-cli side by side (the window finds
# the bridge beside itself), plus README.md and LICENSE when present.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="${1:-$(grep -A3 '^\[workspace.package\]' Cargo.toml | sed -nE 's/^version = "(.*)"/\1/p')}"
version="${version#v}"
bin_dir="${2:-target/release}"
out_dir="${3:-dist}"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
for program in postrinch postrinch-mcp postrinch-cli; do
  if [ ! -f "$bin_dir/$program" ]; then
    echo "package.sh: no $bin_dir/$program: build the three programs with --release first" >&2
    exit 1
  fi
  install -m 755 "$bin_dir/$program" "$stage/$program"
done
for doc in README.md LICENSE; do
  if [ -f "$doc" ]; then cp "$doc" "$stage/"; fi
done

archive="postrinch-${version}-linux-$(uname -m).tar.gz"
mkdir -p "$out_dir"
tar -C "$stage" -czf "$out_dir/$archive" $(cd "$stage" && ls)
echo "Wrote $out_dir/$archive"
tar -tzvf "$out_dir/$archive"
