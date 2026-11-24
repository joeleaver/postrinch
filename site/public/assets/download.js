// Points the download links at the latest release's own files.
//
// Releases are GitHub releases of joeleaver/postrinch, made by the Release
// workflow (.github/workflows/release.yml); their file names carry the
// version, so the page asks GitHub's public API which release is the latest,
// the same question the installed app asks when it checks for updates
// (crates/postrinch-update). Without this script, or when GitHub can't be
// reached, every link still goes to the latest release's page on GitHub.
"use strict";

(function () {
  var API = "https://api.github.com/repos/joeleaver/postrinch/releases/latest";
  // The installed app picks its package by the same ending (check.rs).
  var SUFFIX = "-linux-x86_64.tar.gz";

  function mb(bytes) {
    return Math.round(bytes / 1048576) + " MB";
  }

  function day(iso) {
    var d = new Date(iso);
    if (isNaN(d)) return "";
    return d.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
  }

  function pick(assets) {
    var pkg = null, sig = null;
    for (var i = 0; i < assets.length; i++) {
      if (assets[i].name.slice(-SUFFIX.length) === SUFFIX) { pkg = assets[i]; break; }
    }
    if (!pkg) return null;
    for (var j = 0; j < assets.length; j++) {
      if (assets[j].name === pkg.name + ".sig") { sig = assets[j]; break; }
    }
    return sig ? { pkg: pkg, sig: sig } : null;
  }

  function show(release) {
    var files = pick(release.assets || []);
    if (!files) return;
    var version = String(release.tag_name || "").replace(/^v/, "");
    var when = day(release.published_at);

    var box = document.querySelector("[data-download]");
    if (box) {
      box.querySelector("[data-version]").textContent =
        "Version " + version + (when ? ", " + when : "");
      var link = box.querySelector("[data-package]");
      link.href = files.pkg.browser_download_url;
      link.textContent = "Download for Linux (x86-64) · " + mb(files.pkg.size);
      link.title = files.pkg.name;
      box.querySelector("[data-signature]").href = files.sig.browser_download_url;
    }
    // The commands name the file itself: a glob would match every
    // version someone has downloaded.
    var names = document.querySelectorAll("[data-file]");
    for (var k = 0; k < names.length; k++) names[k].textContent = files.pkg.name;
    var line = document.querySelector("[data-release-line]");
    if (line) line.textContent = "Version " + version + " · Linux x86-64 · keeps itself up to date";
  }

  fetch(API, { headers: { Accept: "application/vnd.github+json" }, referrerPolicy: "no-referrer" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (release) { if (release) show(release); })
    .catch(function () { /* The links already go to GitHub's latest release. */ });
})();
