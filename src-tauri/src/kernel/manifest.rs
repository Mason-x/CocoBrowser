//! Audited kernel download manifests.
//!
//! First-party policy: do **not** trust live GitHub "latest" API for install.
//! Only assets listed here (or a future signed release manifest) may be downloaded.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const EMBEDDED_MANIFEST_JSON: &str = include_str!("../../resources/kernels/manifest.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KernelManifest {
  pub schema_version: u32,
  pub kernels: Vec<KernelAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KernelAsset {
  pub id: String,
  pub version: String,
  pub platform: String,
  pub url: String,
  pub sha256: String,
  pub size: u64,
  pub executable_candidates: Vec<String>,
  /// e.g. `binary-source-delayed` when upstream source lags the binary.
  pub source_status: String,
}

impl KernelManifest {
  pub fn parse_str(json: &str) -> Result<Self, String> {
    serde_json::from_str(json).map_err(|e| format!("Invalid kernel manifest JSON: {e}"))
  }

  pub fn embedded() -> Result<Self, String> {
    Self::parse_str(EMBEDDED_MANIFEST_JSON)
  }

  pub fn load_file(path: &Path) -> Result<Self, String> {
    let text = std::fs::read_to_string(path)
      .map_err(|e| format!("Failed to read kernel manifest {}: {e}", path.display()))?;
    Self::parse_str(&text)
  }

  pub fn find(&self, id: &str, version: &str, platform: &str) -> Option<&KernelAsset> {
    self
      .kernels
      .iter()
      .find(|k| k.id == id && k.version == version && k.platform == platform)
  }

  pub fn validate_asset_integrity_fields(asset: &KernelAsset) -> Result<(), String> {
    if asset.url.is_empty() || !asset.url.starts_with("https://") {
      return Err("Kernel asset URL must be non-empty HTTPS".into());
    }
    if asset.sha256.len() != 64 || !asset.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
      return Err("Kernel asset sha256 must be 64 hex chars".into());
    }
    if asset.size == 0 {
      return Err("Kernel asset size must be > 0".into());
    }
    if asset.executable_candidates.is_empty() {
      return Err("Kernel asset must list executableCandidates".into());
    }
    Ok(())
  }
}

/// Host platform id used in the manifest (`windows-x64`, etc.).
pub fn current_platform_id() -> &'static str {
  #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
  {
    "windows-x64"
  }
  #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
  {
    "macos-arm64"
  }
  #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
  {
    "macos-x64"
  }
  #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
  {
    "linux-x64"
  }
  #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
  {
    "linux-arm64"
  }
  #[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
  )))]
  {
    "unknown"
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn embedded_manifest_parses_and_has_fchromium_windows() {
    let m = KernelManifest::embedded().expect("embedded manifest");
    assert_eq!(m.schema_version, 1);
    let asset = m
      .find("fingerprint-chromium", "148.0.7778.215", "windows-x64")
      .expect("fchromium 148 windows asset");
    assert_eq!(
      asset.sha256,
      "9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579"
    );
    assert_eq!(asset.size, 189_767_686);
    assert!(asset.url.starts_with("https://"));
    assert_eq!(asset.source_status, "source-available");
    KernelManifest::validate_asset_integrity_fields(asset).unwrap();
  }

  #[test]
  fn rejects_non_https_url() {
    let asset = KernelAsset {
      id: "x".into(),
      version: "1".into(),
      platform: "windows-x64".into(),
      url: "http://example.com/a.zip".into(),
      sha256: "a".repeat(64),
      size: 1,
      executable_candidates: vec!["chrome.exe".into()],
      source_status: "ok".into(),
    };
    assert!(KernelManifest::validate_asset_integrity_fields(&asset).is_err());
  }
}
