//! In-app auto-updater backed by GitHub Releases.
//!
//! On startup (and on demand) we ask GitHub for the latest release. If it is
//! newer than the running build we surface a notice with the changelog; the
//! user can then install it in place with one click (the running binary is
//! swapped for the freshly downloaded one).

use anyhow::Result;

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
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Query GitHub for the latest release. Returns `Some` only when it is strictly
/// newer than the running version. Blocking — run on a background thread.
pub fn check() -> Result<Option<UpdateInfo>> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()?
        .fetch()?;

    let Some(latest) = releases.into_iter().next() else {
        return Ok(None);
    };

    if self_update::version::bump_is_greater(current_version(), &latest.version).unwrap_or(false) {
        Ok(Some(UpdateInfo {
            version: latest.version,
            notes: latest.body.unwrap_or_default(),
        }))
    } else {
        Ok(None)
    }
}

/// After an in-place update, rewrite the bundle's `Info.plist` version so the
/// macOS "About" panel (which reads the plist, not the binary) shows the new
/// version. `self_update` only swaps the executable, leaving the plist stale.
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

/// Download the latest release asset for this platform and replace the running
/// binary in place. Blocking — run on a background thread. After this succeeds
/// the app must be restarted to load the new binary.
pub fn install() -> Result<()> {
    self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(REPO_NAME)
        .current_version(current_version())
        .show_download_progress(false)
        .no_confirm(true)
        .build()?
        .update()?;
    Ok(())
}
