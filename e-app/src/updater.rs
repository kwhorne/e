//! In-app auto-updater backed by GitHub Releases.
//!
//! Nothing here touches `api.github.com` on the normal path. The API allows 60
//! unauthenticated calls an hour per address, which a day of restarts — or an
//! office behind one IP — uses up, and then every check fails with a 403. So:
//! the latest version comes from where `github.com/<owner>/<repo>/releases/latest`
//! redirects to, the release notes from `CHANGELOG.md` at that tag, and the
//! asset from its conventional download URL, verified against the `.sha256`
//! published beside it. The API is only a fallback for the version lookup.
//!
//! On startup (throttled) and on demand we look up the latest release. If it is
//! newer than the running build we surface a notice with the changelog; the
//! user can then install it in place with one click (the running binary is
//! swapped for the freshly downloaded one).

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

pub const REPO_OWNER: &str = "kwhorne";
pub const REPO_NAME: &str = "e";

/// Information about an available update.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// Latest version, e.g. `0.2.0` (without a leading `v`).
    pub version: String,
    /// Release notes / changelog body (Markdown).
    pub notes: String,
}

/// Progress of an update operation, surfaced in the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Downloading,
    Installed,
    Failed(String),
    /// The update *check* itself failed (network / GitHub rate limit).
    CheckFailed(String),
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn repo_url() -> String {
    format!("https://github.com/{REPO_OWNER}/{REPO_NAME}")
}

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// `…/releases/tag/v0.9.17` → `0.9.17`. `None` for anything else, including
/// the un-redirected `…/releases/latest`.
pub fn version_from_release_url(url: &str) -> Option<String> {
    let (_, tag) = url.rsplit_once("/releases/tag/")?;
    let tag = tag.trim_end_matches('/');
    let v = tag.strip_prefix('v').unwrap_or(tag);
    v.chars()
        .next()
        .filter(|c| c.is_ascii_digit())
        .map(|_| v.to_string())
}

/// The latest version, from the redirect `releases/latest` answers with.
fn latest_version_via_redirect() -> Result<String> {
    let url = format!("{}/releases/latest", repo_url());
    let resp = ureq::head(&url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .context("asking github.com for the latest release")?;
    version_from_release_url(resp.get_url())
        .ok_or_else(|| anyhow!("unexpected redirect target {}", resp.get_url()))
}

/// The latest version from the GitHub API — the fallback, since it's rationed.
fn latest_version_via_api() -> Result<String> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()?
        .fetch()?;
    releases
        .into_iter()
        .next()
        .map(|r| r.version)
        .ok_or_else(|| anyhow!("no releases published"))
}

/// The `## [version]` section of a Keep-a-Changelog file.
pub fn changelog_section(changelog: &str, version: &str) -> Option<String> {
    let head = format!("## [{version}]");
    let start = changelog.find(&head)?;
    let body_start = changelog[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(changelog.len());
    let end = changelog[body_start..]
        .find("\n## [")
        .map(|i| body_start + i)
        .unwrap_or(changelog.len());
    let section = changelog[body_start..end].trim();
    (!section.is_empty()).then(|| section.to_string())
}

/// Release notes for `version`: its changelog section at the tag (no API).
fn release_notes(version: &str) -> Option<String> {
    let url = format!(
        "https://raw.githubusercontent.com/{REPO_OWNER}/{REPO_NAME}/v{version}/CHANGELOG.md"
    );
    let text = ureq::get(&url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .ok()?
        .into_string()
        .ok()?;
    changelog_section(&text, version)
}

/// What to tell the user when both lookups failed.
fn check_failure(redirect: anyhow::Error, api: anyhow::Error) -> anyhow::Error {
    let api_text = format!("{api:#}");
    if api_text.contains("403") {
        anyhow!(
            "GitHub's API limit for this address is used up (60 calls an hour without a \
             login), and the release page didn't answer either ({redirect:#}). Try again later."
        )
    } else {
        anyhow!("{redirect:#}; API: {api_text}")
    }
}

/// Find the latest release. Returns `Some` only when it is strictly newer than
/// the running version. Blocking — run on a background thread.
pub fn check() -> Result<Option<UpdateInfo>> {
    let latest = match latest_version_via_redirect() {
        Ok(v) => v,
        Err(first) => latest_version_via_api().map_err(|second| check_failure(first, second))?,
    };
    if self_update::version::bump_is_greater(current_version(), &latest).unwrap_or(false) {
        let notes = release_notes(&latest)
            .unwrap_or_else(|| format!("See {}/releases/tag/v{latest}", repo_url()));
        Ok(Some(UpdateInfo {
            version: latest,
            notes,
        }))
    } else {
        Ok(None)
    }
}

/// After an in-place update, rewrite the bundle's `Info.plist` version so the
/// macOS "About" panel (which reads the plist, not the binary) shows the new
/// version. Replacing the executable leaves the plist stale.
pub fn patch_bundle_version(version: &str) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(bundle) = exe
        .ancestors()
        .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
    else {
        return;
    };
    let plist = bundle.join("Contents/Info.plist");
    let Ok(content) = std::fs::read_to_string(&plist) else {
        return;
    };
    let content = replace_plist_string(&content, "CFBundleShortVersionString", version);
    let content = replace_plist_string(&content, "CFBundleVersion", version);
    let _ = std::fs::write(&plist, content);
    // Nudge LaunchServices to pick up the change on next launch.
    let _ = std::process::Command::new("/usr/bin/touch")
        .arg(bundle)
        .status();
}

/// Replace the `<string>` value that follows `<key>{key}</key>` in a plist.
fn replace_plist_string(content: &str, key: &str, value: &str) -> String {
    let needle = format!("<key>{key}</key>");
    let Some(kpos) = content.find(&needle) else {
        return content.to_string();
    };
    let Some(srel) = content[kpos..].find("<string>") else {
        return content.to_string();
    };
    let sstart = kpos + srel + "<string>".len();
    let Some(erel) = content[sstart..].find("</string>") else {
        return content.to_string();
    };
    let eabs = sstart + erel;
    format!("{}{}{}", &content[..sstart], value, &content[eabs..])
}

/// Hex SHA-256 of `bytes`, the way `shasum -a 256` prints it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download `version`'s asset for this platform from its conventional URL,
/// verify it against the published `.sha256`, and replace the running binary in
/// place. Blocking — run on a background thread. After this succeeds the app
/// must be restarted to load the new binary.
pub fn install(version: &str) -> Result<()> {
    let target = self_update::get_target();
    let asset = format!("{REPO_NAME}-{target}.tar.gz");
    let base = format!("{}/releases/download/v{version}", repo_url());
    let dir = std::env::temp_dir().join(format!("e-update-{}-{version}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(&asset);
    {
        let mut file = std::fs::File::create(&archive)?;
        self_update::Download::from_url(&format!("{base}/{asset}"))
            .download_to(&mut file)
            .with_context(|| format!("downloading {asset}"))?;
    }

    // The checksum published beside the asset must match what we got.
    let expected = ureq::get(&format!("{base}/{asset}.sha256"))
        .timeout(HTTP_TIMEOUT)
        .call()
        .with_context(|| format!("downloading {asset}.sha256"))?
        .into_string()?;
    let expected = expected
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let actual = sha256_hex(&std::fs::read(&archive)?);
    if expected.len() != 64 || expected != actual {
        let _ = std::fs::remove_dir_all(&dir);
        bail!("checksum mismatch for {asset}: published {expected}, downloaded {actual}");
    }

    self_update::Extract::from_source(&archive)
        .archive(self_update::ArchiveKind::Tar(Some(
            self_update::Compression::Gz,
        )))
        .extract_file(&dir, REPO_NAME)
        .with_context(|| format!("extracting {REPO_NAME} from {asset}"))?;
    let new_exe = dir.join(REPO_NAME);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new_exe, std::fs::Permissions::from_mode(0o755));
    }
    self_update::self_replace::self_replace(&new_exe)
        .context("replacing the running executable")?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_out_of_the_release_redirect() {
        assert_eq!(
            version_from_release_url("https://github.com/kwhorne/e/releases/tag/v0.9.17"),
            Some("0.9.17".into())
        );
        assert_eq!(
            version_from_release_url("https://github.com/kwhorne/e/releases/tag/0.9.17/"),
            Some("0.9.17".into())
        );
        // Not redirected (no releases), or something else entirely.
        assert_eq!(
            version_from_release_url("https://github.com/kwhorne/e/releases/latest"),
            None
        );
        assert_eq!(
            version_from_release_url("https://github.com/kwhorne/e"),
            None
        );
    }

    #[test]
    fn picks_one_version_out_of_the_changelog() {
        let log = "# Changelog\n\n## [Unreleased]\n\n## [0.9.17] - 2026-09-05\n\n### Fixed\n\n- **A.** text\n\n## [0.9.16] - 2026-09-05\n\n- older\n";
        assert_eq!(
            changelog_section(log, "0.9.17").as_deref(),
            Some("### Fixed\n\n- **A.** text")
        );
        assert_eq!(changelog_section(log, "0.9.16").as_deref(), Some("- older"));
        assert_eq!(changelog_section(log, "0.1.0"), None);
        assert_eq!(changelog_section(log, "Unreleased"), None);
    }

    #[test]
    fn sha256_matches_shasum() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Against github.com — run explicitly with `--ignored`.
    #[test]
    #[ignore]
    fn live_latest_version_comes_from_the_redirect() {
        let v = latest_version_via_redirect().expect("releases/latest redirects");
        assert!(v.starts_with("0.") || v.starts_with('1'), "{v}");
        let notes = release_notes(&v).expect("CHANGELOG.md at the tag");
        assert!(!notes.is_empty());
        eprintln!("latest {v}: {} bytes of notes", notes.len());
    }

    #[test]
    fn plist_string_is_replaced_in_place() {
        let plist = "<key>CFBundleShortVersionString</key>\n<string>0.9.16</string>\n<key>Other</key><string>x</string>";
        let out = replace_plist_string(plist, "CFBundleShortVersionString", "0.9.17");
        assert!(out.contains("<string>0.9.17</string>"));
        assert!(out.contains("<key>Other</key><string>x</string>"));
    }
}
