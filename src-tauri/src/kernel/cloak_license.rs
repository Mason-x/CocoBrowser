//! CloakBrowser license-key storage and official release discovery.
//!
//! The key is protected with the OS account's secret store and is never
//! returned to the frontend. CloakBrowser 150 still contacts the vendor for
//! license validation and session leases; that is an upstream requirement.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::kinds::CLOAK_BROWSER_150;
use super::manifest::current_platform_id;

const LICENSE_PURPOSE: &str = "cloakbrowser-license-key-v1";
const VALIDATE_URL: &str = "https://cloakbrowser.dev/api/license/validate";
const VERSION_URL: &str = "https://cloakbrowser.dev/api/download/version";
const SESSION_COUNT_URL: &str = "https://cloakbrowser.dev/api/license/session/count";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CloakLicenseStatus {
  pub configured: bool,
  pub valid: Option<bool>,
  pub plan: Option<String>,
  pub expires: Option<String>,
  pub active_sessions: Option<u32>,
  pub session_limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CloakLatestRelease {
  pub id: String,
  pub version: String,
  pub platform: String,
  pub source_status: String,
}

#[derive(Debug, Deserialize)]
struct LicenseResponse {
  #[serde(default)]
  valid: bool,
  #[serde(default = "default_plan")]
  plan: String,
  expires: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionCountResponse {
  active: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
  version: String,
}

fn default_plan() -> String {
  "free".to_string()
}

fn license_path() -> PathBuf {
  crate::app_dirs::data_subdir().join("cloakbrowser_license.key")
}

fn json_error(code: &str) -> String {
  serde_json::json!({ "code": code }).to_string()
}

fn json_error_detail(code: &str, detail: impl ToString) -> String {
  serde_json::json!({
    "code": code,
    "params": { "detail": detail.to_string() }
  })
  .to_string()
}

fn http_client() -> Result<reqwest::Client, String> {
  reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(10))
    .read_timeout(Duration::from_secs(20))
    .build()
    .map_err(|e| e.to_string())
}

pub(crate) fn load_license_key() -> Result<Option<String>, String> {
  let path = license_path();
  let protected = match fs::read_to_string(&path) {
    Ok(value) => value,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(error.to_string()),
  };
  let plaintext = crate::secret_store::unprotect_local(LICENSE_PURPOSE, protected.trim())?;
  let key = String::from_utf8(plaintext).map_err(|e| e.to_string())?;
  let key = key.trim().to_string();
  if key.is_empty() {
    Ok(None)
  } else {
    Ok(Some(key))
  }
}

fn save_license_key(key: &str) -> Result<(), String> {
  let path = license_path();
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let protected = crate::secret_store::protect_local(LICENSE_PURPOSE, key.as_bytes())?;
  let tmp = path.with_extension("key.tmp");
  fs::write(&tmp, protected).map_err(|e| e.to_string())?;
  super::install_registry::replace_file(&tmp, &path)
}

async fn validate_remote(key: &str) -> Result<LicenseResponse, String> {
  let response = http_client()?
    .post(VALIDATE_URL)
    .json(&serde_json::json!({ "license_key": key }))
    .send()
    .await
    .map_err(|e| e.to_string())?
    .error_for_status()
    .map_err(|e| e.to_string())?;
  response.json().await.map_err(|e| e.to_string())
}

async fn active_session_count(key: &str) -> Option<u32> {
  let client = http_client().ok()?;
  let response = client
    .post(SESSION_COUNT_URL)
    .json(&serde_json::json!({ "license_key": key }))
    .send()
    .await
    .ok()?
    .error_for_status()
    .ok()?;
  response.json::<SessionCountResponse>().await.ok()?.active
}

pub(crate) async fn require_valid_license_key() -> Result<String, String> {
  let key = load_license_key()
    .map_err(|e| json_error_detail("CLOAK_LICENSE_STORAGE_FAILED", e))?
    .ok_or_else(|| json_error("CLOAK_LICENSE_KEY_REQUIRED"))?;
  let info = validate_remote(&key)
    .await
    .map_err(|e| json_error_detail("CLOAK_LICENSE_SERVER_UNAVAILABLE", e))?;
  if !info.valid {
    return Err(json_error("CLOAK_LICENSE_INVALID"));
  }
  Ok(key)
}

pub(crate) async fn fetch_latest_release() -> Result<CloakLatestRelease, String> {
  let platform = current_platform_id();
  if platform != "windows-x64" {
    return Err(json_error("CLOAK_PLATFORM_UNSUPPORTED"));
  }
  let response = http_client()
    .map_err(|e| json_error_detail("CLOAK_RELEASE_LOOKUP_FAILED", e))?
    .get(VERSION_URL)
    .header("X-Platform", platform)
    .send()
    .await
    .map_err(|e| json_error_detail("CLOAK_RELEASE_LOOKUP_FAILED", e))?
    .error_for_status()
    .map_err(|e| json_error_detail("CLOAK_RELEASE_LOOKUP_FAILED", e))?;
  let version = response
    .json::<VersionResponse>()
    .await
    .map_err(|e| json_error_detail("CLOAK_RELEASE_LOOKUP_FAILED", e))?
    .version;
  if !version.starts_with("150.") {
    return Err(json_error_detail(
      "CLOAK_RELEASE_LOOKUP_FAILED",
      format!("expected the v150 channel, received {version}"),
    ));
  }
  Ok(CloakLatestRelease {
    id: CLOAK_BROWSER_150.to_string(),
    version,
    platform: platform.to_string(),
    source_status: "proprietary-binary".to_string(),
  })
}

#[tauri::command]
pub async fn get_cloak_license_status(refresh: Option<bool>) -> Result<CloakLicenseStatus, String> {
  let Some(key) =
    load_license_key().map_err(|e| json_error_detail("CLOAK_LICENSE_STORAGE_FAILED", e))?
  else {
    return Ok(CloakLicenseStatus {
      configured: false,
      valid: None,
      plan: None,
      expires: None,
      active_sessions: None,
      session_limit: None,
    });
  };

  if !refresh.unwrap_or(false) {
    return Ok(CloakLicenseStatus {
      configured: true,
      valid: None,
      plan: None,
      expires: None,
      active_sessions: None,
      session_limit: None,
    });
  }

  let info = validate_remote(&key)
    .await
    .map_err(|e| json_error_detail("CLOAK_LICENSE_SERVER_UNAVAILABLE", e))?;
  let active_sessions = if info.valid {
    active_session_count(&key).await
  } else {
    None
  };
  let session_limit = match info.plan.as_str() {
    "free" => Some(1),
    _ => None,
  };
  Ok(CloakLicenseStatus {
    configured: true,
    valid: Some(info.valid),
    plan: Some(info.plan),
    expires: info.expires,
    active_sessions,
    session_limit,
  })
}

#[tauri::command]
pub async fn set_cloak_license_key(key: String) -> Result<CloakLicenseStatus, String> {
  let key = key.trim();
  if key.is_empty() {
    return Err(json_error("CLOAK_LICENSE_KEY_REQUIRED"));
  }
  let info = validate_remote(key)
    .await
    .map_err(|e| json_error_detail("CLOAK_LICENSE_SERVER_UNAVAILABLE", e))?;
  if !info.valid {
    return Err(json_error("CLOAK_LICENSE_INVALID"));
  }
  save_license_key(key).map_err(|e| json_error_detail("CLOAK_LICENSE_STORAGE_FAILED", e))?;
  let active_sessions = active_session_count(key).await;
  let session_limit = if info.plan == "free" { Some(1) } else { None };
  Ok(CloakLicenseStatus {
    configured: true,
    valid: Some(true),
    plan: Some(info.plan),
    expires: info.expires,
    active_sessions,
    session_limit,
  })
}

#[tauri::command]
pub fn clear_cloak_license_key() -> Result<(), String> {
  let path = license_path();
  match fs::remove_file(path) {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(error) => Err(json_error_detail("CLOAK_LICENSE_STORAGE_FAILED", error)),
  }
}

#[tauri::command]
pub async fn get_cloak_latest_release() -> Result<CloakLatestRelease, String> {
  fetch_latest_release().await
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn key_is_stored_protected_and_never_returned() {
    let tmp = TempDir::new().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());
    save_license_key("cb_secret_value").unwrap();
    let on_disk = fs::read_to_string(license_path()).unwrap();
    assert!(!on_disk.contains("cb_secret_value"));
    assert_eq!(
      load_license_key().unwrap().as_deref(),
      Some("cb_secret_value")
    );
  }
}
