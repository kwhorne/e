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
//! user can then install it in place with one click.
//!
//! Inside a macOS app bundle the whole bundle is replaced, never just its
//! executable: swapping `Contents/MacOS/e` (or rewriting `Info.plist`) breaks
//! the bundle's code signature, and with it Gatekeeper's trust, the Keychain
//! items e may read and the privacy permissions granted to it. The release
//! carries the notarized app as a zip, and it is only put in place once
//! `codesign` confirms it is signed by e's Developer ID and is the version
//! asked for. A bare binary (Linux, or run outside a bundle) is swapped for
//! the archive the release workflow builds, as before.

use std::path::{Path, PathBuf};
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

/// The `<string>` value that follows `<key>{key}</key>` in a plist.
#[cfg(target_os = "macos")]
fn plist_string<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let kpos = content.find(&format!("<key>{key}</key>"))?;
    let sstart = kpos + content[kpos..].find("<string>")? + "<string>".len();
    let send = sstart + content[sstart..].find("</string>")?;
    Some(content[sstart..send].trim())
}

/// Hex SHA-256 of `bytes`, the way `shasum -a 256` prints it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download `{base}/{asset}` into `dir` and check it against the `.sha256`
/// published beside it.
fn download_verified(base: &str, asset: &str, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(asset);
    {
        let mut file = std::fs::File::create(&path)?;
        self_update::Download::from_url(&format!("{base}/{asset}"))
            .download_to(&mut file)
            .with_context(|| format!("downloading {asset}"))?;
    }
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
    let actual = sha256_hex(&std::fs::read(&path)?);
    if expected.len() != 64 || expected != actual {
        bail!("checksum mismatch for {asset}: published {expected}, downloaded {actual}");
    }
    Ok(path)
}

/// The macOS app bundle the running executable lives in, if any.
#[cfg(target_os = "macos")]
pub fn running_bundle() -> Option<PathBuf> {
    let exe = crate::platform::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// Install `version`. Blocking — run on a background thread. After this
/// succeeds the app must be restarted to load the new version.
pub fn install(version: &str) -> Result<()> {
    let base = format!("{}/releases/download/v{version}", repo_url());
    #[cfg(target_os = "macos")]
    if let Some(bundle) = running_bundle() {
        return install_bundle(&base, version, &bundle);
    }
    install_binary(&base, version)
}

/// Replace the running binary with the one in this platform's archive.
fn install_binary(base: &str, version: &str) -> Result<()> {
    let target = self_update::get_target();
    let asset = format!("{REPO_NAME}-{target}.tar.gz");
    let dir = std::env::temp_dir().join(format!("e-update-{}-{version}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = (|| {
        let archive = download_verified(base, &asset, &dir)?;
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
            .context("replacing the running executable")
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The release asset holding the signed, notarized app bundle.
#[cfg(target_os = "macos")]
pub fn bundle_asset(version: &str) -> String {
    format!("{REPO_NAME}-{version}-universal.app.zip")
}

/// The designated requirement of a Developer ID application signed by e's
/// team: Apple's anchor, the Developer ID intermediate and leaf, team
/// 52AT44QFCM. Anyone can sign a bundle; only that team can satisfy this.
#[cfg(target_os = "macos")]
const SIGNED_BY_E: &str = "anchor apple generic \
    and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
    and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
    and certificate leaf[subject.OU] = \"52AT44QFCM\"";

/// Replace the whole app bundle at `bundle` with the release's signed one.
#[cfg(target_os = "macos")]
fn install_bundle(base: &str, version: &str, bundle: &Path) -> Result<()> {
    let parent = bundle
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent folder", bundle.display()))?;
    // Staged beside the bundle, so the final moves are renames on one volume.
    let stage = parent.join(format!(".e-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir(&stage).with_context(|| {
        format!(
            "e can't write to {}; download the new version from {}/releases/latest",
            parent.display(),
            repo_url()
        )
    })?;
    let result = (|| {
        let zip = download_verified(base, &bundle_asset(version), &stage)?;
        // ditto keeps the signature, extended attributes and stapled ticket.
        let out = std::process::Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&stage)
            .output()
            .context("running ditto")?;
        if !out.status.success() {
            bail!(
                "unpacking {}: {}",
                bundle_asset(version),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let new_app = stage.join(format!("{REPO_NAME}.app"));
        verify_bundle(&new_app, version, SIGNED_BY_E)?;
        swap_bundle(bundle, &new_app, &stage.join("previous.app"))
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Check that `app` satisfies the code-signing `requirement` — every file in
/// it sealed, nothing added — and is `version`, so an older signed release
/// can't be passed off as the new one.
#[cfg(target_os = "macos")]
fn verify_bundle(app: &Path, version: &str, requirement: &str) -> Result<()> {
    let out = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R={requirement}"))
        .arg(app)
        .output()
        .context("running codesign")?;
    if !out.status.success() {
        bail!(
            "the downloaded app is not signed by e's developer: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist"))
        .context("reading the downloaded app's Info.plist")?;
    match plist_string(&plist, "CFBundleShortVersionString") {
        Some(v) if v == version => Ok(()),
        other => bail!(
            "the downloaded app is version {}, not {version}",
            other.unwrap_or("?")
        ),
    }
}

/// Put `new` where `current` is, moving `current` to `aside` first and back
/// if the second move fails. Both renames stay on one volume.
#[cfg(target_os = "macos")]
fn swap_bundle(current: &Path, new: &Path, aside: &Path) -> Result<()> {
    std::fs::rename(current, aside)
        .with_context(|| format!("moving {} aside", current.display()))?;
    if let Err(e) = std::fs::rename(new, current) {
        let _ = std::fs::rename(aside, current);
        return Err(e).with_context(|| format!("putting the new app at {}", current.display()));
    }
    // Let LaunchServices see the new bundle's Info.plist.
    let _ = std::process::Command::new("/usr/bin/touch")
        .arg(current)
        .status();
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

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_a_string_out_of_a_plist() {
        let plist = "<key>CFBundleShortVersionString</key>\n<string>0.9.16</string>\n<key>Other</key><string>x</string>";
        assert_eq!(
            plist_string(plist, "CFBundleShortVersionString"),
            Some("0.9.16")
        );
        assert_eq!(plist_string(plist, "Other"), Some("x"));
        assert_eq!(plist_string(plist, "Missing"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_bundle_asset_is_the_universal_app_zip() {
        assert_eq!(bundle_asset("0.20.0"), "e-0.20.0-universal.app.zip");
    }

    /// A minimal app bundle around a copy of `/usr/bin/true`, ad-hoc signed.
    #[cfg(target_os = "macos")]
    fn adhoc_bundle(dir: &Path, version: &str) -> PathBuf {
        let app = dir.join("e.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/e")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>CFBundleIdentifier</key><string>dev.e.test</string>\
                 <key>CFBundleExecutable</key><string>e</string>\
                 <key>CFBundleShortVersionString</key><string>{version}</string>\
                 </dict></plist>\n"
            ),
        )
        .unwrap();
        let ok = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .status()
            .unwrap()
            .success();
        assert!(ok, "ad-hoc signing the test bundle");
        app
    }

    #[cfg(target_os = "macos")]
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("e-updater-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_not_signed_by_e_is_refused() {
        let dir = scratch("unsigned");
        let app = adhoc_bundle(&dir, "0.20.0");
        let err = verify_bundle(&app, "0.20.0", SIGNED_BY_E).unwrap_err();
        assert!(
            format!("{err:#}").contains("not signed by e's developer"),
            "{err:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_tampered_or_older_bundle_is_refused() {
        let dir = scratch("tampered");
        let app = adhoc_bundle(&dir, "0.20.0");
        // Signature intact, the requirement aside: the version must match.
        assert!(verify_bundle(&app, "0.20.0", "always").is_ok());
        let err = verify_bundle(&app, "0.21.0", "always").unwrap_err();
        assert!(
            format!("{err:#}").contains("version 0.20.0, not 0.21.0"),
            "{err:#}"
        );
        // A file added after signing breaks the seal.
        std::fs::write(app.join("Contents/MacOS/extra"), "x").unwrap();
        assert!(verify_bundle(&app, "0.20.0", "always").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against an installed release — run with `--ignored` on a Mac with e
    /// in /Applications: our own signature must satisfy the requirement.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn an_installed_release_satisfies_the_requirement() {
        let app = Path::new("/Applications/e.app");
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        let version = plist_string(&plist, "CFBundleShortVersionString").unwrap();
        verify_bundle(app, version, SIGNED_BY_E).unwrap();
        let other_team = SIGNED_BY_E.replace("52AT44QFCM", "AAAAAAAAAA");
        assert!(verify_bundle(app, version, &other_team).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn swapping_puts_the_new_bundle_in_place() {
        let dir = scratch("swap");
        let current = dir.join("e.app");
        let new = dir.join("stage/e.app");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("v"), "old").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("v"), "new").unwrap();
        swap_bundle(&current, &new, &dir.join("stage/previous.app")).unwrap();
        assert_eq!(std::fs::read_to_string(current.join("v")).unwrap(), "new");
        // A failed second move puts the old bundle back.
        let missing = dir.join("stage/nothing.app");
        assert!(swap_bundle(&current, &missing, &dir.join("stage/aside.app")).is_err());
        assert_eq!(std::fs::read_to_string(current.join("v")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
