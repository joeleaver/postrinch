# postrinch.jkbase.app

Postrinch's public site: what it does, how to get it, how to set up Gmail sign-in, and the
privacy policy. A plain static [jkbase](https://jkbase.app) site: the pages are `public/`, there
is no build step, it sets no cookies and loads nothing from anyone else, except that the
download box asks GitHub's API which release is the latest.

| Page | What it is |
|---|---|
| `public/index.html` | Features, screenshots, download |
| `public/gmail-setup.html` | Making a Google OAuth "Desktop app" client of your own (the early-preview sign-in) |
| `public/privacy.html` | The privacy policy (LBME Publishing). Google's app verification links to it |
| `public/postrinch-update-key.pem` | The release key, for checking a download's signature by hand |

## Downloads need no deploy

The download links come from GitHub, not from this site. Releases are GitHub releases of
`joeleaver/postrinch`, made by the Release workflow on a `v*` tag; their files carry the
version (`postrinch-<v>-linux-x86_64.tar.gz` and its `.sig`). `public/assets/download.js` asks
`api.github.com/repos/joeleaver/postrinch/releases/latest` from the visitor's browser and points
the button and signature link at that release's files, picking the package by the same ending
the app's updater does (`crates/postrinch-update/src/check.rs`). Without JavaScript, or when
GitHub can't be reached, the links go to the latest release's page on GitHub.

So a new release shows on the site at once. Deploy only when a page changes.

When Windows or macOS packages arrive, add their endings to `download.js` and a box for each to
`index.html`.

## Deploy

Joe approves each deploy. Then:

```bash
site/deploy.sh check   # checks only
site/deploy.sh         # checks, then `jkbase deploy` from site/
```

The checks: `public/postrinch-update-key.pem` is the key builds carry
(`crates/postrinch-update/update-key.pub`), and every `/…` link in a page names a file under
`public/`. `jkbase deploy` uses the credentials in `~/.jkbase` (`jkbase login` makes them).
`jkbase deployments` lists what's been deployed; `jkbase rollback` goes back one.

The project was made once with `jkbase project create postrinch`, which gives
`https://postrinch.jkbase.app`.

## Screenshots

`public/assets/shots/*.png` are taken from a made-up demo store, never from real mail: see
`tools/demo-window.sh`. Each is 1440×900, the size the pages' `<img>` tags declare. Retake them
when the window changes enough that they mislead.

## The release key

If the update key ever changes (`tools/update-key.sh`, which strands every install), remake the
PEM, or `deploy.sh` refuses:

```bash
{ printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'
  base64 -d crates/postrinch-update/update-key.pub; } |
  openssl pkey -pubin -inform DER -out site/public/postrinch-update-key.pem
```

## Keep the pages true

The features, the setup guide and the privacy policy describe the app. When a change makes
them wrong (a new account type, something new Postrinch sends anywhere, a new scope, Google
verification done so the setup guide is no longer needed), change the page in the same PR and
deploy it. The privacy policy promises that anything new sent anywhere is said in the app
first, and its date changes with it.
