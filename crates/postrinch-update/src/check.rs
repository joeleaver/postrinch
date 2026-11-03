//! The check: read the latest GitHub release, and when it is newer, download
//! this system's package and its signature, verify it, and unpack it into
//! the install's `update/` folder. Nothing outside `update/` changes.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::{CURRENT, PROGRAMS, STAGE, exe, newer_than_current};

/// The latest release of Postrinch, as GitHub's API describes it.
const FEED: &str = "https://api.github.com/repos/joeleaver/postrinch/releases/latest";
/// Points the check at another feed of the same shape (for tests).
const FEED_ENV: &str = "POSTRINCH_UPDATE_FEED";
/// A GitHub token to read releases with; without it, `gh auth token`.
const TOKEN_ENV: &str = "POSTRINCH_GITHUB_TOKEN";
/// The Ed25519 key packages are signed with, the raw 32 bytes in base64
/// (`tools/update-key.sh` writes it).
const PUBLIC_KEY: &str = include_str!("../update-key.pub");

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    /// The API's address for the asset: with a token and
    /// `Accept: application/octet-stream`, its bytes, from a private
    /// repository too.
    url: String,
    browser_download_url: String,
}

/// The ending of this system's package: `-linux-x86_64.tar.gz`.
fn package_suffix() -> String {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("-{}-{}.{ext}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The version staged in `dir`, if a whole one is.
pub fn staged_version(dir: &Path) -> Option<String> {
    let stage = dir.join(STAGE);
    let ready: serde_json::Value =
        serde_json::from_slice(&std::fs::read(stage.join("ready.json")).ok()?).ok()?;
    let version = ready.get("version")?.as_str()?.to_owned();
    PROGRAMS
        .iter()
        .all(|name| stage.join(exe(name)).is_file())
        .then_some(version)
}

/// Check once, and stage a newer verified package in `dir`. Answers the
/// version that is staged, if one is.
pub fn check_and_stage(dir: &Path) -> Result<Option<String>, String> {
    let feed = std::env::var(FEED_ENV).unwrap_or_else(|_| FEED.to_owned());
    let http = Http::new()?;
    let release: Release = serde_json::from_slice(&http.get(&feed, "application/vnd.github+json")?)
        .map_err(|e| format!("reading the latest release: {e}"))?;
    let version = release.tag_name.trim_start_matches('v').to_owned();
    if !newer_than_current(&version) {
        return Ok(None);
    }
    if staged_version(dir).as_deref() == Some(version.as_str()) {
        return Ok(Some(version));
    }
    let (package, signature) = pick(&release.assets).ok_or_else(|| {
        format!(
            "{version} has no package for this system (*{})",
            package_suffix()
        )
    })?;
    let bytes = http.download(&package)?;
    verify(PUBLIC_KEY, &bytes, &http.download(&signature)?)?;
    stage(dir, &version, &bytes)?;
    Ok(Some(version))
}

/// This system's package and its signature.
fn pick(assets: &[Asset]) -> Option<(Asset, Asset)> {
    let suffix = package_suffix();
    let package = assets.iter().find(|a| a.name.ends_with(&suffix))?;
    let signature = assets
        .iter()
        .find(|a| a.name == format!("{}.sig", package.name))?;
    Some((package.clone(), signature.clone()))
}

/// Unpack `package` into `dir/update` and mark it ready.
fn stage(dir: &Path, version: &str, package: &[u8]) -> Result<(), String> {
    let stage = dir.join(STAGE);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("creating {}: {e}", stage.display()))?;
    let archive = stage.join(format!("package{}", package_suffix()));
    std::fs::write(&archive, package).map_err(|e| format!("saving the package: {e}"))?;
    // The system's tar (Windows 10 and later ship one that reads zip too).
    let unpacked = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage)
        .status()
        .map_err(|e| format!("running tar: {e}"))?;
    let _ = std::fs::remove_file(&archive);
    let whole = PROGRAMS.iter().all(|name| stage.join(exe(name)).is_file());
    if !unpacked.success() || !whole {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!(
            "the {version} package does not hold all three programs"
        ));
    }
    let ready = serde_json::json!({ "version": version }).to_string();
    std::fs::write(stage.join("ready.json"), ready)
        .map_err(|e| format!("marking the update ready: {e}"))
}

/// Does `signature` sign `package` with `public_key`?
fn verify(public_key: &str, package: &[u8], signature: &[u8]) -> Result<(), String> {
    use base64::Engine as _;
    use ed25519_dalek::{Signature, VerifyingKey};
    let key: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(public_key.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or("this build has no update key (tools/update-key.sh)")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|e| format!("the update key: {e}"))?;
    let signature = Signature::from_slice(signature).map_err(|e| format!("the signature: {e}"))?;
    key.verify_strict(package, &signature)
        .map_err(|_| "the package's signature does not verify".to_owned())
}

struct Http {
    client: reqwest::blocking::Client,
    token: Option<String>,
}

impl Http {
    fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("postrinch/{CURRENT}"))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            token: token(),
        })
    }

    /// GET `url`. The token goes only to GitHub's API; GitHub's redirect
    /// to its file storage drops it.
    fn get(&self, url: &str, accept: &str) -> Result<Vec<u8>, String> {
        let mut request = self.client.get(url).header("Accept", accept);
        if let Some(token) = self
            .token
            .as_ref()
            .filter(|_| url.starts_with("https://api.github.com/"))
        {
            request = request.bearer_auth(token);
        }
        let response = request.send().map_err(|e| format!("{url}: {e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND
            && self.token.is_none()
            && url.starts_with("https://api.github.com/")
        {
            return Err("GitHub has no release to show without signing in: sign in with `gh auth login`, or set POSTRINCH_GITHUB_TOKEN".into());
        }
        if !status.is_success() {
            return Err(format!("{url}: {status}"));
        }
        response
            .bytes()
            .map(|b| b.to_vec())
            .map_err(|e| format!("{url}: {e}"))
    }

    fn download(&self, asset: &Asset) -> Result<Vec<u8>, String> {
        if self.token.is_some() {
            self.get(&asset.url, "application/octet-stream")
        } else {
            self.get(&asset.browser_download_url, "application/octet-stream")
        }
    }
}

/// A token to read releases with: `POSTRINCH_GITHUB_TOKEN`, else the GitHub
/// CLI's sign-in. The repository is private for now; without either, a
/// public one still works.
fn token() -> Option<String> {
    if let Ok(token) = std::env::var(TOKEN_ENV) {
        return Some(token.trim().to_owned()).filter(|t| !t.is_empty());
    }
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    let token = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key_pair() -> (SigningKey, String) {
        use base64::Engine as _;
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public =
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().to_bytes());
        (signing, public)
    }

    #[test]
    fn a_package_signed_with_the_key_verifies_and_nothing_else_does() {
        let (signing, public) = key_pair();
        let package = b"postrinch 1.2.3";
        let signature = signing.sign(package).to_bytes();
        assert!(verify(&public, package, &signature).is_ok());
        assert!(verify(&public, b"postrinch 1.2.4", &signature).is_err());
        let other = SigningKey::from_bytes(&[8u8; 32]).sign(package).to_bytes();
        assert!(verify(&public, package, &other).is_err());
        assert!(verify(&public, package, &signature[..63]).is_err());
    }

    #[test]
    fn a_build_without_a_key_refuses_every_update() {
        let (signing, _) = key_pair();
        let signature = signing.sign(b"x").to_bytes();
        let refused = verify("not a key", b"x", &signature).unwrap_err();
        assert!(refused.contains("no update key"), "{refused}");
    }

    #[test]
    fn the_compiled_in_key_is_a_key() {
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(PUBLIC_KEY.trim())
            .unwrap();
        assert_eq!(raw.len(), 32);
    }

    #[test]
    fn this_systems_package_is_picked_with_its_signature() {
        let asset = |name: &str| Asset {
            name: name.into(),
            url: format!("api/{name}"),
            browser_download_url: format!("dl/{name}"),
        };
        let suffix = package_suffix();
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v9.0.0",
            "assets": [
                {"name": "postrinch-9.0.0-plan9-mips.tar.gz", "url": "a", "browser_download_url": "b"},
                {"name": format!("postrinch-9.0.0{suffix}"), "url": "c", "browser_download_url": "d"},
                {"name": format!("postrinch-9.0.0{suffix}.sig"), "url": "e", "browser_download_url": "f"},
            ],
        }))
        .unwrap();
        let (package, signature) = pick(&release.assets).unwrap();
        assert_eq!((package.url.as_str(), signature.url.as_str()), ("c", "e"));
        assert!(
            pick(&[asset(&format!("x{suffix}"))]).is_none(),
            "no signature, no package"
        );
    }

    #[test]
    fn a_stage_counts_only_when_all_three_programs_are_there() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join(STAGE);
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("ready.json"), r#"{"version":"9.0.0"}"#).unwrap();
        std::fs::write(stage.join(exe("postrinch")), "").unwrap();
        std::fs::write(stage.join(exe("postrinch-mcp")), "").unwrap();
        assert_eq!(staged_version(dir.path()), None);
        std::fs::write(stage.join(exe("postrinch-cli")), "").unwrap();
        assert_eq!(staged_version(dir.path()).as_deref(), Some("9.0.0"));
    }
}
