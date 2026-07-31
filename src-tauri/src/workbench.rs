//! The local landing page a profile opens on.
//!
//! The page reports what a site actually sees from inside the browser — exit IP
//! and its geolocation, timezone, language, user agent, screen, WebGL — and
//! compares it against what the persona was configured to present. A launch that
//! silently disagrees with its own configuration is the failure worth catching,
//! and only the browser itself can report the observed side.
//!
//! Delivered as an unpacked extension whose service worker navigates the tab the
//! browser starts on, once that worker is running.
//!
//! Overriding `chrome://newtab` was tried first and does not work: Chromium
//! navigates the startup tab before an unpacked extension finishes registering,
//! so the override consistently arrives too late and the user is left on an empty
//! default new tab. Declaring the override also raises Chromium's "this page was
//! changed by an extension" bubble, which hands the user a button that turns the
//! page off. Navigating from inside the worker wins that race instead of losing
//! it, and needs no override and therefore no bubble.
//!
//! Two other alternatives were tried and rejected. A loopback HTTP server cannot
//! work: profiles launch with `--proxy-bypass-list=<-loopback>`, which
//! deliberately *removes* Chromium's implicit localhost bypass so loopback
//! traffic also goes through the proxy and a page can neither reach local
//! services nor detect that localhost is exempt — a `http://127.0.0.1` page is
//! sent upstream and fails. A `file://` launch URL renders, but its origin is
//! opaque, so the page cannot `fetch` the expected values written beside it.
//!
//! The extension declares no permissions and no `web_accessible_resources`, so a
//! visited page cannot probe for it. Navigating the active tab needs neither:
//! the `tabs` permission only gates *reading* a tab's URL and title.
//!
//! The HTML is built by the frontend and written here rather than generated in
//! Rust, so every label stays in the locale files instead of being duplicated
//! into the backend.

use std::path::PathBuf;

use crate::profile::ProfileManager;

const PAGE_FILE_NAME: &str = "workbench.html";
const SCRIPT_FILE_NAME: &str = "workbench.js";
const EXPECTED_FILE_NAME: &str = "expected.json";
const WORKER_FILE_NAME: &str = "worker.js";

/// Opens the page on the tab the browser started on.
///
/// Only ever reached through `onInstalled` and `onStartup`. A bare top-level
/// call also works, but the worker can in principle be revived later in a
/// session, and navigating a tab the user is actually reading would be far worse
/// than not showing this page at all. Both events fire early enough on their
/// own — verified on a fresh profile, a reused profile, and a 1.5s window.
///
/// Lives in Rust rather than beside the page in the frontend because it holds no
/// user-facing text: nothing here needs translating.
const WORKER_JS: &str = r#"const open = async () => {
  try {
    const url = chrome.runtime.getURL("workbench.html");
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (tab && tab.id !== undefined) await chrome.tabs.update(tab.id, { url });
  } catch (error) {
    console.error("Failed to open the environment check page", error);
  }
};
chrome.runtime.onInstalled.addListener(open);
chrome.runtime.onStartup.addListener(open);
"#;

/// Lives beside the profile's browser data rather than inside it: Chromium
/// rewrites the user data directory, and an unpacked extension has to stay
/// readable for as long as the browser runs.
fn extension_dir(profile_id: &str) -> Result<PathBuf, String> {
  let profile = ProfileManager::instance()
    .list_profiles()
    .map_err(|e| e.to_string())?
    .into_iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| serde_json::json!({ "code": "PROFILE_NOT_FOUND" }).to_string())?;

  Ok(
    ProfileManager::instance()
      .get_profiles_dir()
      .join(profile.id.to_string())
      .join("workbench-ext"),
  )
}

/// Manifest for the landing page.
///
/// MV3, no permissions, no host access, no web-accessible resources, and no
/// `chrome_url_overrides` — the worker is the only moving part.
fn manifest_json() -> String {
  serde_json::json!({
    "manifest_version": 3,
    "name": "Environment check",
    "version": "1.0",
    "background": { "service_worker": WORKER_FILE_NAME }
  })
  .to_string()
}

/// What to load and open when a profile launches, if a page was written and the
/// setting is on.
///
/// `None` opens the browser with its normal new tab page, which is the right
/// outcome for a missing page — never a failed launch.
pub(crate) fn extension_if_enabled(profile_id: &str) -> Option<String> {
  let enabled = crate::settings_manager::SettingsManager::instance()
    .load_settings()
    .map(|s| s.show_workbench_page)
    .unwrap_or(true);
  if !enabled {
    return None;
  }
  let dir = extension_dir(profile_id).ok()?;
  if !dir.join(PAGE_FILE_NAME).exists() {
    return None;
  }
  Some(dir.to_string_lossy().to_string())
}

/// Record what the persona will actually present, for the page to compare against.
///
/// The frontend builds the page before the launch command runs, so the values
/// baked into it are the persona as it stood *before* the geo gate — and the gate
/// is precisely what rewrites the timezone and language of a persona that follows
/// its exit. Comparing against those stale values either flagged nothing or
/// flagged the wrong thing, which is why the page reads this file instead.
///
/// Best effort by design: the page falls back to its baked-in values, and a
/// launch must never fail over its landing page.
pub(crate) fn write_expected_values(
  extension_dir: &str,
  persona: &crate::kernel::persona::FingerprintPersona,
) {
  let expected = serde_json::json!({
    "timezone": persona.timezone,
    "language": persona.language,
    "acceptLanguages": persona.accept_languages,
  });
  let path = std::path::Path::new(extension_dir).join(EXPECTED_FILE_NAME);
  if let Err(error) = std::fs::write(&path, expected.to_string()) {
    log::warn!("Failed to write the workbench expected values to {path:?}: {error}");
  }
}

/// Write the workbench page as an unpacked extension for a profile.
///
/// The script is a separate file, not an inline block: MV3 extension pages run
/// under `script-src 'self'`, which silently refuses inline scripts — the page
/// rendered but every value stayed blank.
#[tauri::command]
pub fn write_workbench_page(profile_id: String, html: String, js: String) -> Result<(), String> {
  let dir = extension_dir(&profile_id)?;
  std::fs::create_dir_all(&dir)
    .map_err(|e| format!("Failed to create the workbench extension directory: {e}"))?;

  std::fs::write(dir.join("manifest.json"), manifest_json())
    .map_err(|e| format!("Failed to write the workbench manifest: {e}"))?;
  std::fs::write(dir.join(PAGE_FILE_NAME), html)
    .map_err(|e| format!("Failed to write the workbench page: {e}"))?;
  std::fs::write(dir.join(SCRIPT_FILE_NAME), js)
    .map_err(|e| format!("Failed to write the workbench script: {e}"))?;
  std::fs::write(dir.join(WORKER_FILE_NAME), WORKER_JS)
    .map_err(|e| format!("Failed to write the workbench worker: {e}"))?;
  // The previous launch's expected values describe the exit that launch went
  // through. Leaving them behind would have this launch compare itself against a
  // different exit if it never reaches the geo gate; without the file the page
  // falls back to the persona it was built from, which is at least this profile.
  let _ = std::fs::remove_file(dir.join(EXPECTED_FILE_NAME));
  crate::app_dirs::restrict_to_owner(&dir);

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_manifest_runs_a_worker_and_overrides_nothing() {
    let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert_eq!(m["background"]["service_worker"], WORKER_FILE_NAME);
    // Chromium navigates the startup tab before an unpacked extension has
    // registered, so an override never reaches it — and declaring one raises the
    // "this page was changed by an extension" bubble, whose "Change it back"
    // button turns the page off for good.
    assert!(
      m.get("chrome_url_overrides").is_none(),
      "the new tab page must not be overridden"
    );
  }

  #[test]
  fn the_manifest_asks_for_nothing() {
    // Permissions or web-accessible resources would make the extension visible
    // to visited pages, which is the opposite of the point. Navigating the
    // active tab needs neither — `tabs` only gates reading a tab's URL.
    let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
    for key in [
      "permissions",
      "host_permissions",
      "web_accessible_resources",
      "content_scripts",
    ] {
      assert!(m.get(key).is_none(), "{key} must not be declared");
    }
  }

  #[test]
  fn the_worker_only_acts_on_install_and_startup() {
    // A bare top-level call also opens the page, but the worker can be revived
    // mid-session, and navigating a tab the user is reading is worse than not
    // showing this page at all.
    assert!(WORKER_JS.contains("chrome.runtime.onInstalled.addListener(open)"));
    assert!(WORKER_JS.contains("chrome.runtime.onStartup.addListener(open)"));
    for line in WORKER_JS.lines() {
      assert_ne!(
        line.trim(),
        "open();",
        "the worker must not navigate on revival"
      );
    }
    // Reusing the tab that is already open; an earlier attempt used tabs.create
    // and left a duplicate behind.
    assert!(WORKER_JS.contains("chrome.tabs.update"));
    assert!(!WORKER_JS.contains("chrome.tabs.create"));
  }

  #[test]
  fn a_profile_that_was_never_written_loads_no_extension() {
    assert!(extension_if_enabled("00000000-0000-4000-8000-000000000000").is_none());
  }

  #[test]
  fn the_expected_values_record_the_persona_the_launch_settled_on() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut persona =
      crate::kernel::persona::FingerprintPersona::auto_consistent_windows("148.0.7778.215")
        .expect("a Windows host builds an auto persona");
    // What the geo gate does to a persona that follows its exit.
    persona.timezone = "Asia/Tokyo".into();
    persona.language = "ja-JP".into();
    persona.accept_languages = vec!["ja-JP".into(), "ja".into()];

    write_expected_values(&dir.path().to_string_lossy(), &persona);

    let written: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(dir.path().join(EXPECTED_FILE_NAME)).unwrap())
        .unwrap();
    assert_eq!(written["timezone"], "Asia/Tokyo");
    assert_eq!(written["language"], "ja-JP");
    assert_eq!(written["acceptLanguages"][1], "ja");
  }
}
