#!/usr/bin/env bash
# Deploy postrinch.jkbase.app (README.md beside this file says when and how).
#
#   site/deploy.sh check     check the site, deploy nothing
#   site/deploy.sh [args…]   check, then `jkbase deploy [args…]` from site/
#
# The checks: the release key the site offers is the one builds carry
# (crates/postrinch-update/update-key.pub), and every local file a page links
# to exists. The download links themselves need no deploy: the page asks
# GitHub for the latest release (public/assets/download.js).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
public="$here/public"
die() { echo "deploy.sh: $*" >&2; exit 1; }

# The key as builds carry it (raw 32 bytes, base64) in the PEM form openssl
# reads: the fixed Ed25519 SubjectPublicKeyInfo prefix, then the key.
check_key() {
  local want
  want="$({ printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'
            base64 -d "$repo/crates/postrinch-update/update-key.pub"; } |
          openssl pkey -pubin -inform DER)"
  [ "$want" = "$(cat "$public/postrinch-update-key.pem")" ] ||
    die "public/postrinch-update-key.pem is not crates/postrinch-update/update-key.pub; remake it (README.md)"
}

# Every link in a page is another site (https:), mail (mailto:), or a file
# under public/ named from the root ("/assets/site.css", "/privacy.html"),
# optionally with a #fragment that must be an id in that page. The pages
# use nothing else, so anything else is refused rather than left unchecked.
links_in() {
  grep -oE "(href|src|srcset)=(\"|')[^\"']*" "$1" | sed -E "s/^(href|src|srcset)=[\"']//"
}

check_links() {
  local bad=0 page link path fragment target
  for page in "$public"/*.html; do
    while read -r link; do
      case "$link" in
        https://*|mailto:*) continue ;;
        /*|\#*) ;;
        *) echo "$(basename "$page"): $link is not root-relative, https: or mailto:" >&2; bad=1; continue ;;
      esac
      path="${link%%#*}"; path="${path%%\?*}"; path="${path%% *}"
      fragment=""; [ "$link" != "${link#*#}" ] && fragment="${link#*#}"
      case "$path" in
        "") target="$page" ;;
        /) target="$public/index.html" ;;
        *) target="$public$path" ;;
      esac
      if [ ! -f "$target" ]; then
        echo "$(basename "$page"): $link names no file under public/" >&2; bad=1
      elif [ -n "$fragment" ] && ! grep -q "id=\"$fragment\"" "$target"; then
        echo "$(basename "$page"): $link names no id=\"$fragment\" in $(basename "$target")" >&2; bad=1
      fi
    done < <(links_in "$page")
  done
  [ "$bad" -eq 0 ] || die "fix the links above before deploying"
}

command -v openssl >/dev/null || die "openssl is required"
check_key
check_links
echo "site checks passed"

[ "${1:-}" = "check" ] && exit 0
command -v jkbase >/dev/null || die "the jkbase CLI is required (and ~/.jkbase credentials)"
cd "$here" && jkbase deploy "$@"
