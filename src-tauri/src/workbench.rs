//! The local landing page a profile opens on.
//!
//! The page reports what a site actually sees from inside the browser — exit IP
//! and its geolocation, timezone, language, user agent, screen, WebGL — and
//! compares it against what the persona was configured to present. A launch that
//! silently disagrees with its own configuration is the failure worth catching,
//! and only the browser itself can report the observed side.
//!
//! Delivered as an unpacked extension that overrides the new tab page, which is
//! also the tab the browser starts on — so the page appears with an empty address
//! bar and no launch URL is involved.
//!
//! Two alternatives were tried and rejected. A loopback HTTP server cannot work:
//! profiles launch with `--proxy-bypass-list=<-loopback>`, which deliberately
//! *removes* Chromium's implicit localhost bypass so loopback traffic also goes
//! through the proxy and a page can neither reach local services nor detect that
//! localhost is exempt — a `http://127.0.0.1` page is sent upstream and fails.
//! A `file://` launch URL works but needs an exception to the launch URL
//! allowlist and leaves the path sitting in the address bar.
//!
//! The extension declares no permissions and no `web_accessible_resources`, so a
//! visited page cannot probe for it.
//!
//! The HTML is built by the frontend and written here rather than generated in
//! Rust, so every label stays in the locale files instead of being duplicated
//! into the backend.

use std::path::PathBuf;

use crate::profile::ProfileManager;

const PAGE_FILE_NAME: &str = "workbench.html";
const SCRIPT_FILE_NAME: &str = "workbench.js";

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

/// Manifest for the new-tab override.
///
/// MV3, no permissions, no host access, no web-accessible resources — it exists
/// only to put a local page behind `chrome://newtab`.
fn manifest_json() -> String {
  serde_json::json!({
    "manifest_version": 3,
    "name": "Environment check",
    "version": "1.0",
    "chrome_url_overrides": { "newtab": PAGE_FILE_NAME }
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

/// Write the workbench page as a new-tab-override extension for a profile.
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
  crate::app_dirs::restrict_to_owner(&dir);

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_manifest_overrides_the_new_tab_page() {
    let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert_eq!(m["chrome_url_overrides"]["newtab"], PAGE_FILE_NAME);
  }

  #[test]
  fn the_manifest_asks_for_nothing() {
    // Permissions or web-accessible resources would make the extension visible
    // to visited pages, which is the opposite of the point.
    let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
    for key in [
      "permissions",
      "host_permissions",
      "web_accessible_resources",
      "content_scripts",
      // A background worker raced the override and left a duplicate tab behind
      // every time it lost; the override reaches the startup tab by itself.
      "background",
    ] {
      assert!(m.get(key).is_none(), "{key} must not be declared");
    }
  }

  #[test]
  fn a_profile_that_was_never_written_loads_no_extension() {
    assert!(extension_if_enabled("00000000-0000-4000-8000-000000000000").is_none());
  }
}
