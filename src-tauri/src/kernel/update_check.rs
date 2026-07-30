//! Upstream release detection for audited kernels.
//!
//! **This module never downloads or installs a kernel.** It reads upstream tag
//! names so the UI can report that a newer release exists, and it reports which
//! *audited* manifest entries are not installed yet. Installing remains
//! restricted to [`super::manifest::KernelManifest::embedded`] entries with a
//! fixed SHA-256 (plan section 5.2 / risk R1): a release that upstream has
//! published but that nobody has audited into the manifest is surfaced as
//! information only, with no install path.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::install_registry::InstallRegistryFile;
use super::manifest::{current_platform_id, KernelManifest};

const UPSTREAM_RELEASES_URL: &str =
  "https://api.github.com/repos/adryfish/fingerprint-chromium/releases/latest";
const UPSTREAM_REPO_HTML: &str = "https://github.com/adryfish/fingerprint-chromium/releases";
const CHECK_CACHE_FILE: &str = "kernel_update_check.json";
const CACHE_TTL_SECS: u64 = 6 * 60 * 60;
const REQUEST_TIMEOUT_SECS: u64 = 15;
/// The upstream payload is a single release object; anything larger is bogus.
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KernelUpdateStatus {
  pub kernel_id: String,
  pub platform: String,
  /// Versions present in the local install registry, newest first.
  pub installed_versions: Vec<String>,
  /// Newest version in the embedded (audited) manifest for this platform.
  pub latest_audited: Option<String>,
  /// Newest audited version that is not installed yet — safe to install.
  pub audited_not_installed: Option<String>,
  /// Newest tag observed upstream. `None` when the check failed or was skipped.
  pub latest_upstream: Option<String>,
  /// True when upstream is ahead of every audited manifest entry. This is
  /// informational: such a version cannot be installed until it is audited.
  pub upstream_ahead_of_audited: bool,
  /// Where a human can review the upstream release notes.
  pub upstream_url: String,
  pub checked_at: u64,
  /// Set when the upstream probe failed; the rest of the struct is still valid.
  pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCheck {
  latest_upstream: Option<String>,
  checked_at: u64,
}

#[derive(Debug, Deserialize)]
struct UpstreamRelease {
  #[serde(default)]
  tag_name: String,
  #[serde(default)]
  draft: bool,
  #[serde(default)]
  prerelease: bool,
}

fn now_secs() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

fn cache_path() -> std::path::PathBuf {
  crate::app_dirs::cache_dir().join(CHECK_CACHE_FILE)
}

fn read_cache() -> Option<CachedCheck> {
  let text = std::fs::read_to_string(cache_path()).ok()?;
  let cached: CachedCheck = serde_json::from_str(&text).ok()?;
  if now_secs().saturating_sub(cached.checked_at) > CACHE_TTL_SECS {
    return None;
  }
  Some(cached)
}

fn write_cache(cached: &CachedCheck) {
  let path = cache_path();
  if let Some(parent) = path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }
  if let Ok(text) = serde_json::to_string(cached) {
    let _ = std::fs::write(path, text);
  }
}

/// Compare dotted numeric versions (`148.0.7778.215`). Missing components read
/// as 0, and any non-numeric component falls back to a string comparison so a
/// malformed upstream tag can never panic or spuriously claim to be newer.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
  use std::cmp::Ordering;
  let mut left = a.split('.');
  let mut right = b.split('.');
  loop {
    match (left.next(), right.next()) {
      (None, None) => return Ordering::Equal,
      (l, r) => {
        let ls = l.unwrap_or("0");
        let rs = r.unwrap_or("0");
        match (ls.parse::<u64>(), rs.parse::<u64>()) {
          (Ok(lv), Ok(rv)) => match lv.cmp(&rv) {
            Ordering::Equal => continue,
            other => return other,
          },
          // A well-formed numeric component always outranks a malformed one, so
          // a garbage upstream tag can never be reported as newer than an
          // audited version (which would show a bogus "update available").
          (Ok(_), Err(_)) => return Ordering::Greater,
          (Err(_), Ok(_)) => return Ordering::Less,
          (Err(_), Err(_)) => match ls.cmp(rs) {
            Ordering::Equal => continue,
            other => return other,
          },
        }
      }
    }
  }
}

/// Strip a leading `v` so `v149.0.1` and `149.0.1` compare equal.
fn normalize_tag(tag: &str) -> String {
  tag.trim().trim_start_matches('v').to_string()
}

fn newest<'a, I: Iterator<Item = &'a str>>(versions: I) -> Option<String> {
  versions
    .max_by(|a, b| compare_versions(a, b))
    .map(str::to_string)
}

async fn fetch_latest_upstream() -> Result<Option<String>, String> {
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
    .build()
    .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

  let response = client
    .get(UPSTREAM_RELEASES_URL)
    // GitHub rejects API requests without a User-Agent.
    .header("User-Agent", "local-fingerprint-browser")
    .header("Accept", "application/vnd.github+json")
    .send()
    .await
    .map_err(|e| format!("Upstream release check failed: {e}"))?;

  if !response.status().is_success() {
    return Err(format!(
      "Upstream release check returned HTTP {}",
      response.status().as_u16()
    ));
  }

  let body = response
    .bytes()
    .await
    .map_err(|e| format!("Failed to read upstream response: {e}"))?;
  if body.len() > MAX_RESPONSE_BYTES {
    return Err("Upstream release response was unexpectedly large".to_string());
  }

  let release: UpstreamRelease =
    serde_json::from_slice(&body).map_err(|e| format!("Invalid upstream release JSON: {e}"))?;

  if release.draft || release.prerelease || release.tag_name.trim().is_empty() {
    return Ok(None);
  }
  Ok(Some(normalize_tag(&release.tag_name)))
}

/// Build the update status for `kernel_id`.
///
/// `force` bypasses the cached upstream probe. When the probe fails the status
/// is still returned with local (audited/installed) information and `error` set,
/// so a offline machine keeps a usable kernels page.
pub async fn check_kernel_updates(kernel_id: &str, force: bool) -> KernelUpdateStatus {
  let platform = current_platform_id().to_string();

  let manifest = KernelManifest::embedded().ok();
  let audited: Vec<String> = manifest
    .as_ref()
    .map(|m| {
      m.kernels
        .iter()
        .filter(|k| k.id == kernel_id && k.platform == platform)
        .map(|k| k.version.clone())
        .collect()
    })
    .unwrap_or_default();

  let registry = InstallRegistryFile::load();
  let mut installed_versions: Vec<String> = registry
    .list_for_id(kernel_id)
    .into_iter()
    .map(|k| k.version.clone())
    .collect();
  installed_versions.sort_by(|a, b| compare_versions(b, a));

  let latest_audited = newest(audited.iter().map(String::as_str));

  // Newest audited entry that is not installed — this one is safe to offer.
  let audited_not_installed = newest(
    audited
      .iter()
      .filter(|v| !installed_versions.contains(v))
      .map(String::as_str),
  );

  let (latest_upstream, error) = if force {
    match fetch_latest_upstream().await {
      Ok(v) => {
        write_cache(&CachedCheck {
          latest_upstream: v.clone(),
          checked_at: now_secs(),
        });
        (v, None)
      }
      Err(e) => (None, Some(e)),
    }
  } else if let Some(cached) = read_cache() {
    (cached.latest_upstream, None)
  } else {
    match fetch_latest_upstream().await {
      Ok(v) => {
        write_cache(&CachedCheck {
          latest_upstream: v.clone(),
          checked_at: now_secs(),
        });
        (v, None)
      }
      Err(e) => (None, Some(e)),
    }
  };

  let upstream_ahead_of_audited = match (&latest_upstream, &latest_audited) {
    (Some(up), Some(aud)) => compare_versions(up, aud) == std::cmp::Ordering::Greater,
    // With nothing audited yet, any upstream release is "ahead".
    (Some(_), None) => true,
    _ => false,
  };

  KernelUpdateStatus {
    kernel_id: kernel_id.to_string(),
    platform,
    installed_versions,
    latest_audited,
    audited_not_installed,
    latest_upstream,
    upstream_ahead_of_audited,
    upstream_url: UPSTREAM_REPO_HTML.to_string(),
    checked_at: now_secs(),
    error,
  }
}

#[tauri::command]
pub async fn check_kernel_updates_command(
  kernel_id: Option<String>,
  force: Option<bool>,
) -> Result<KernelUpdateStatus, String> {
  let id = kernel_id.unwrap_or_else(|| "fingerprint-chromium".to_string());
  Ok(check_kernel_updates(&id, force.unwrap_or(false)).await)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::cmp::Ordering;

  #[test]
  fn compares_dotted_numeric_versions() {
    assert_eq!(
      compare_versions("149.0.7845.100", "148.0.7778.215"),
      Ordering::Greater
    );
    assert_eq!(
      compare_versions("148.0.7778.215", "148.0.7778.215"),
      Ordering::Equal
    );
    assert_eq!(
      compare_versions("148.0.7778.9", "148.0.7778.215"),
      Ordering::Less
    );
  }

  #[test]
  fn treats_missing_components_as_zero() {
    assert_eq!(compare_versions("148", "148.0.0.0"), Ordering::Equal);
    assert_eq!(compare_versions("148.1", "148.0.9999"), Ordering::Greater);
  }

  #[test]
  fn malformed_component_does_not_panic_and_is_not_newer() {
    // A garbage tag must never compare Greater than a real audited version.
    assert_ne!(compare_versions("abc", "148.0.7778.215"), Ordering::Greater);
  }

  #[test]
  fn normalizes_v_prefixed_tags() {
    assert_eq!(normalize_tag("v149.0.1"), "149.0.1");
    assert_eq!(normalize_tag("  149.0.1  "), "149.0.1");
  }

  #[test]
  fn newest_picks_highest_version() {
    let v = vec!["148.0.1", "149.0.2", "147.9.9"];
    assert_eq!(newest(v.into_iter()), Some("149.0.2".to_string()));
  }

  #[test]
  fn embedded_manifest_versions_are_comparable() {
    // Guards against a manifest entry whose version cannot be ordered, which
    // would silently disable the "audited but not installed" offer.
    let manifest = KernelManifest::embedded().expect("embedded manifest must parse");
    for asset in &manifest.kernels {
      assert_ne!(
        compare_versions(&asset.version, "0.0.0.0"),
        Ordering::Less,
        "manifest version {} did not order above zero",
        asset.version
      );
    }
  }
}
