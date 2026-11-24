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

# Every link in a page (href, src, each srcset candidate) is another site
# (https:), mail (mailto:), or a file under public/ named from the root
# ("/assets/site.css", "/privacy.html"), optionally with a #fragment that
# must be an element id in that page. The pages use nothing else, so
# anything else is refused rather than left unchecked. Parsed as HTML, not
# matched with patterns, so quoting and srcset lists can't hide a link.
check_links() {
  PUBLIC="$public" python3 -I - <<'PY' || die "fix the links above before deploying"
import os, sys
from html.parser import HTMLParser
from urllib.parse import urlsplit

public = os.environ["PUBLIC"]

class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids = [], set()
    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if value is None:
                continue
            if name == "id":
                self.ids.add(value)
            elif name in ("href", "src"):
                self.links.append(value.strip())
            elif name == "srcset":
                self.links += [c.strip().split()[0] for c in value.split(",") if c.strip()]

def parse(path):
    page = Page()
    with open(path, encoding="utf-8") as f:
        page.feed(f.read())
    return page

pages = {name: parse(os.path.join(public, name))
         for name in sorted(os.listdir(public)) if name.endswith(".html")}
bad = 0
for name, page in pages.items():
    for link in page.links:
        if link.startswith(("https://", "mailto:")):
            continue
        if not link.startswith(("/", "#")):
            print(f"{name}: {link} is not root-relative, https: or mailto:", file=sys.stderr); bad = 1; continue
        url = urlsplit(link)
        path = url.path or "/" + name
        if path.endswith("/"):
            path += "index.html"
        target = os.path.join(public, path.lstrip("/"))
        if not os.path.isfile(target):
            print(f"{name}: {link} names no file under public/", file=sys.stderr); bad = 1
        elif url.fragment:
            ids = pages[path.lstrip("/")].ids if path.lstrip("/") in pages else parse(target).ids
            if url.fragment not in ids:
                print(f"{name}: {link} names no id \"{url.fragment}\"", file=sys.stderr); bad = 1
sys.exit(bad)
PY
}

command -v openssl >/dev/null || die "openssl is required"
command -v python3 >/dev/null || die "python3 is required"
check_key
check_links
echo "site checks passed"

[ "${1:-}" = "check" ] && exit 0
command -v jkbase >/dev/null || die "the jkbase CLI is required (and ~/.jkbase credentials)"
cd "$here" && jkbase deploy "$@"
