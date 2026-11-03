#!/usr/bin/env bash
# Makes the key pair release packages are signed with (DESIGN.md, "Installing
# and updating"): the private half goes into the repository's
# POSTRINCH_UPDATE_SIGNING_KEY secret, where only the Release workflow reads
# it, and the public half into crates/postrinch-update/update-key.pub, which
# every build compiles in. Run it once and commit update-key.pub. Running it
# again replaces the key: copies built with the old one then refuse every
# update until they are reinstalled.
#
# Needs openssl 3 and an authenticated gh.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

key="$(mktemp)"
trap 'rm -f "$key"' EXIT
openssl genpkey -algorithm ed25519 -out "$key"
gh secret set POSTRINCH_UPDATE_SIGNING_KEY --repo joeleaver/postrinch < "$key"
# The DER public key ends with the raw 32 bytes.
openssl pkey -in "$key" -pubout -outform DER | tail -c 32 | base64 > crates/postrinch-update/update-key.pub
echo "Stored the private key as the POSTRINCH_UPDATE_SIGNING_KEY secret (it is not kept here)."
echo "Wrote crates/postrinch-update/update-key.pub: $(cat crates/postrinch-update/update-key.pub). Commit it."
