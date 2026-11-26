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
  // Each system's box (data-download="<system>"), the file its button
  // downloads, and the package beside it, if it has one. Linux's and the
  // Windows zip are picked by the same endings the installed app updates
  // from (check.rs); the Setup installs that zip's programs.
  var SYSTEMS = {
    linux: { name: "Linux (x86-64)", main: "-linux-x86_64.tar.gz" },
    windows: { name: "Windows (x86-64)", main: "-windows-x86_64-setup.exe", also: "-windows-x86_64.zip" }
  };

  function mb(bytes) {
    return Math.round(bytes / 1048576) + " MB";
  }

  function day(iso) {
    var d = new Date(iso);
    if (isNaN(d)) return "";
    return d.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
  }

  function named(assets, name) {
    for (var i = 0; i < assets.length; i++) {
      if (assets[i].name === name) return assets[i];
    }
    return null;
  }

  // The file ending `suffix`, with its signature; none without one.
  function pick(assets, suffix) {
    for (var i = 0; i < assets.length; i++) {
      var name = assets[i].name;
      if (name.slice(-suffix.length) === suffix) {
        var sig = named(assets, name + ".sig");
        return sig ? { file: assets[i], sig: sig } : null;
      }
    }
    return null;
  }

  function showSystem(box, system, assets, version, when) {
    var main = pick(assets, system.main);
    if (!main) return false;
    box.querySelector("[data-version]").textContent =
      "Version " + version + (when ? ", " + when : "");
    var link = box.querySelector("[data-package]");
    link.href = main.file.browser_download_url;
    link.textContent = "Download for " + system.name + " · " + mb(main.file.size);
    link.title = main.file.name;
    box.querySelector("[data-signature]").href = main.sig.browser_download_url;
    var also = system.also && pick(assets, system.also);
    var alsoLink = box.querySelector("[data-also]");
    if (also && alsoLink) {
      alsoLink.href = also.file.browser_download_url;
      alsoLink.title = also.file.name;
      box.querySelector("[data-also-signature]").href = also.sig.browser_download_url;
    }
    // The commands name the file itself: a glob would match every
    // version someone has downloaded.
    var names = document.querySelectorAll("[data-file='" + box.getAttribute("data-download") + "']");
    for (var k = 0; k < names.length; k++) names[k].textContent = main.file.name;
    return true;
  }

  function show(release) {
    var assets = release.assets || [];
    var version = String(release.tag_name || "").replace(/^v/, "");
    var when = day(release.published_at);
    var shown = [];
    var boxes = document.querySelectorAll("[data-download]");
    for (var i = 0; i < boxes.length; i++) {
      var system = SYSTEMS[boxes[i].getAttribute("data-download")];
      if (system && showSystem(boxes[i], system, assets, version, when)) shown.push(system.name);
    }
    var line = document.querySelector("[data-release-line]");
    if (line && shown.length) {
      line.textContent = "Version " + version + " · " + shown.join(", ") + " · keeps itself up to date";
    }
  }

  fetch(API, { headers: { Accept: "application/vnd.github+json" }, referrerPolicy: "no-referrer" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (release) { if (release) show(release); })
    .catch(function () { /* The links already go to GitHub's latest release. */ });
})();
