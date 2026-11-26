# Packages the three release-built programs for Windows, as the Release
# workflow attaches them to a GitHub release (DESIGN.md, "Installing and
# updating"), so the same step can be run and inspected on a Windows machine.
# The counterpart of tools/package.sh.
#
# Usage: pwsh tools/windows/package.ps1 [-Version X.Y.Z] [-BinDir target/release] [-OutDir dist]
#
#   Version  Defaults to the workspace version in Cargo.toml.
#   BinDir   Not built here: run
#            `cargo build --release -p postrinch -p postrinch-mcp -p postrinch-cli`
#            first, WITHOUT the `debug` feature.
#   OutDir   Defaults to dist.
#
# Writes <OutDir>/postrinch-<version>-windows-x86_64.zip, the package the
# installed app updates itself from (crates/postrinch-update/src/check.rs
# picks it by that ending): postrinch.exe, postrinch-mcp.exe and
# postrinch-cli.exe side by side (the window finds the bridge beside itself),
# README.md, and the licenses that travel with every copy. It also fills
# <OutDir>/installer-stage with the same files and the install marker, for
# tools/windows/postrinch.iss.
param(
    [string]$Version = "",
    [string]$BinDir = "target/release",
    [string]$OutDir = "dist"
)
$ErrorActionPreference = "Stop"
Set-Location (Resolve-Path "$PSScriptRoot/../..")

if (-not $Version) {
    $manifest = Get-Content Cargo.toml -Raw
    if ($manifest -notmatch '(?ms)^\[workspace\.package\].*?^version = "([^"]+)"') {
        throw "package.ps1: Cargo.toml names no [workspace.package] version"
    }
    $Version = $Matches[1]
}
$Version = $Version.TrimStart("v")

$programs = "postrinch.exe", "postrinch-mcp.exe", "postrinch-cli.exe"
$docs = "README.md", "LICENSE-MIT", "LICENSE-APACHE", "crates/postrinch/assets/fonts/LICENSE-IBM-Plex.txt"
foreach ($program in $programs) {
    if (-not (Test-Path "$BinDir/$program" -PathType Leaf)) {
        throw "package.ps1: no $BinDir/${program}: build the three programs with --release first"
    }
}
foreach ($doc in $docs) {
    if (-not (Test-Path $doc -PathType Leaf)) { throw "package.ps1: no ${doc}: every package carries it" }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$name = "postrinch-$Version-windows-x86_64"
$stage = Join-Path $OutDir $name
$installer = Join-Path $OutDir "installer-stage"
foreach ($dir in $stage, $installer) {
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    New-Item -ItemType Directory -Path $dir | Out-Null
    foreach ($program in $programs) { Copy-Item "$BinDir/$program" $dir }
    foreach ($doc in $docs) { Copy-Item $doc $dir }
}
# Only the Setup's copy is an install: the zip is unpacked anywhere, and
# `postrinch.exe --install` from there writes the marker where it installs.
Set-Content -Path (Join-Path $installer "postrinch-install.json") -Value "{`"version`":`"$Version`"}" -NoNewline -Encoding ascii

# The files at the zip's top level, as the updater's `tar -xf` expects.
$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Remove-Item -Recurse -Force $stage
Write-Host "Wrote $zip"
& "$env:SystemRoot\System32\tar.exe" -tvf $zip
if ($LASTEXITCODE -ne 0) { throw "package.ps1: the system's tar cannot read $zip" }
