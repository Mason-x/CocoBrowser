use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, create_dir_all};
use std::path::{Path, PathBuf};

use crate::kernel::install_registry::InstallRegistryFile;
use crate::kernel::manifest::{current_platform_id, KernelManifest};
use crate::profile::types::{get_host_os, BrowserProfile, SyncMode};
use crate::profile::ProfileManager;

const TARGET_KERNEL: &str = "fingerprint-chromium";
const AUDITED_KERNEL_VERSION: &str = "148.0.7778.215";
const MAX_IMPORT_ENTRIES: usize = 200_000;
const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_PREFERENCES_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DetectedProfile {
  pub browser: String,
  pub mapped_browser: String,
  pub name: String,
  pub path: String,
  pub description: String,
}

fn map_browser_type(browser: &str) -> Result<&'static str, String> {
  match browser.trim().to_ascii_lowercase().as_str() {
    "chrome" | "edge" | "chromium" => Ok(TARGET_KERNEL),
    _ => Err(
      serde_json::json!({
        "code": "IMPORT_SOURCE_UNSUPPORTED",
        "params": { "browser": browser }
      })
      .to_string(),
    ),
  }
}

pub struct ProfileImporter {
  base_dirs: BaseDirs,
  profile_manager: &'static ProfileManager,
}

impl ProfileImporter {
  fn new() -> Self {
    Self {
      base_dirs: BaseDirs::new().expect("Failed to get base directories"),
      profile_manager: ProfileManager::instance(),
    }
  }

  pub fn instance() -> &'static ProfileImporter {
    &PROFILE_IMPORTER
  }

  pub fn detect_existing_profiles(
    &self,
  ) -> Result<Vec<DetectedProfile>, Box<dyn std::error::Error>> {
    let mut detected_profiles = Vec::new();

    // Import is intentionally limited to the three profile formats audited for
    // migration into fingerprint-chromium. Chromium-derived products are not
    // assumed compatible merely because they contain a Preferences file.
    detected_profiles.extend(self.detect_chrome_profiles()?);
    detected_profiles.extend(self.detect_edge_profiles()?);
    detected_profiles.extend(self.detect_chromium_profiles()?);

    let mut seen_paths = HashSet::new();
    let unique_profiles: Vec<DetectedProfile> = detected_profiles
      .into_iter()
      .filter(|profile| seen_paths.insert(profile.path.clone()))
      .collect();

    Ok(unique_profiles)
  }

  fn detect_chrome_profiles(&self) -> Result<Vec<DetectedProfile>, Box<dyn std::error::Error>> {
    let mut profiles = Vec::new();

    #[cfg(target_os = "macos")]
    {
      let chrome_dir = self
        .base_dirs
        .home_dir()
        .join("Library/Application Support/Google/Chrome");
      profiles.extend(self.scan_chrome_profiles_dir(&chrome_dir, "chrome")?);
    }

    #[cfg(target_os = "windows")]
    {
      let local_app_data = self.base_dirs.data_local_dir();
      let chrome_dir = local_app_data.join("Google/Chrome/User Data");
      profiles.extend(self.scan_chrome_profiles_dir(&chrome_dir, "chrome")?);
    }

    #[cfg(target_os = "linux")]
    {
      let chrome_dir = self.base_dirs.home_dir().join(".config/google-chrome");
      profiles.extend(self.scan_chrome_profiles_dir(&chrome_dir, "chrome")?);
    }

    Ok(profiles)
  }

  fn detect_chromium_profiles(&self) -> Result<Vec<DetectedProfile>, Box<dyn std::error::Error>> {
    let mut profiles = Vec::new();

    #[cfg(target_os = "macos")]
    {
      let chromium_dir = self
        .base_dirs
        .home_dir()
        .join("Library/Application Support/Chromium");
      profiles.extend(self.scan_chrome_profiles_dir(&chromium_dir, "chromium")?);
    }

    #[cfg(target_os = "windows")]
    {
      let local_app_data = self.base_dirs.data_local_dir();
      let chromium_dir = local_app_data.join("Chromium/User Data");
      profiles.extend(self.scan_chrome_profiles_dir(&chromium_dir, "chromium")?);
    }

    #[cfg(target_os = "linux")]
    {
      let chromium_dir = self.base_dirs.home_dir().join(".config/chromium");
      profiles.extend(self.scan_chrome_profiles_dir(&chromium_dir, "chromium")?);
    }

    Ok(profiles)
  }

  fn detect_edge_profiles(&self) -> Result<Vec<DetectedProfile>, Box<dyn std::error::Error>> {
    let mut profiles = Vec::new();

    #[cfg(target_os = "macos")]
    {
      let edge_dir = self
        .base_dirs
        .home_dir()
        .join("Library/Application Support/Microsoft Edge");
      profiles.extend(self.scan_chrome_profiles_dir(&edge_dir, "edge")?);
    }

    #[cfg(target_os = "windows")]
    {
      let local_app_data = self.base_dirs.data_local_dir();
      let edge_dir = local_app_data.join("Microsoft/Edge/User Data");
      profiles.extend(self.scan_chrome_profiles_dir(&edge_dir, "edge")?);
    }

    #[cfg(target_os = "linux")]
    {
      let edge_dir = self.base_dirs.home_dir().join(".config/microsoft-edge");
      profiles.extend(self.scan_chrome_profiles_dir(&edge_dir, "edge")?);
    }

    Ok(profiles)
  }

  fn scan_chrome_profiles_dir(
    &self,
    browser_dir: &Path,
    browser_type: &str,
  ) -> Result<Vec<DetectedProfile>, Box<dyn std::error::Error>> {
    let mut profiles = Vec::new();
    let mapped_browser = map_browser_type(browser_type)?;

    if !browser_dir.exists() {
      return Ok(profiles);
    }

    let default_profile = browser_dir.join("Default");
    if default_profile.exists() && default_profile.join("Preferences").exists() {
      profiles.push(DetectedProfile {
        browser: browser_type.to_string(),
        mapped_browser: mapped_browser.to_string(),
        name: format!(
          "{} - Default Profile",
          self.get_browser_display_name(browser_type)
        ),
        path: default_profile.to_string_lossy().to_string(),
        description: "Default profile".to_string(),
      });
    }

    if let Ok(entries) = fs::read_dir(browser_dir) {
      for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
          let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

          if dir_name.starts_with("Profile ") && path.join("Preferences").exists() {
            let profile_number = &dir_name[8..];
            profiles.push(DetectedProfile {
              browser: browser_type.to_string(),
              mapped_browser: mapped_browser.to_string(),
              name: format!(
                "{} - Profile {}",
                self.get_browser_display_name(browser_type),
                profile_number
              ),
              path: path.to_string_lossy().to_string(),
              description: format!("Profile {profile_number}"),
            });
          }
        }
      }
    }

    Ok(profiles)
  }

  fn get_browser_display_name(&self, browser_type: &str) -> &str {
    match browser_type {
      "chrome" => "Google Chrome",
      "edge" => "Microsoft Edge",
      "chromium" => "Chromium",
      TARGET_KERNEL => "Fingerprint Chromium",
      _ => "Unknown Browser",
    }
  }

  pub async fn import_profile(
    &self,
    source_path: &str,
    browser_type: &str,
    new_profile_name: &str,
    proxy_id: Option<String>,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let mapped = map_browser_type(browser_type)?;
    debug_assert_eq!(mapped, TARGET_KERNEL);
    let source_path = Self::validate_source_profile(Path::new(source_path), browser_type)?;

    if new_profile_name.trim().is_empty() {
      return Err(
        serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" })
          .to_string()
          .into(),
      );
    }
    crate::kernel::geo_consistency::reject_cloud_proxy_id(proxy_id.as_deref())?;

    let version = self.get_audited_kernel_version()?;
    let persona = crate::kernel::persona::FingerprintPersona::auto_consistent_windows(&version)
      .map_err(|e| format!("Failed to create fingerprint persona: {e}"))?;
    persona.validate(&version)?;

    let existing_profiles = self.profile_manager.list_profiles()?;
    if existing_profiles
      .iter()
      .any(|p| p.name.eq_ignore_ascii_case(new_profile_name.trim()))
    {
      return Err(format!("Profile with name '{new_profile_name}' already exists").into());
    }

    let profile_id = uuid::Uuid::new_v4();
    let profiles_dir = self.profile_manager.get_profiles_dir();
    create_dir_all(&profiles_dir)?;
    let new_profile_uuid_dir = profiles_dir.join(profile_id.to_string());
    let new_profile_data_dir = new_profile_uuid_dir.join("profile");
    let new_default_profile_dir = new_profile_data_dir.join("Default");

    // The UUID directory is the import transaction boundary. Any copy or
    // metadata failure removes only this newly generated directory.
    create_dir_all(&new_profile_uuid_dir)?;
    let import_result = (|| -> Result<(), Box<dyn std::error::Error>> {
      // Detected/manual sources point at a Chrome-family profile directory
      // (usually `Default` or `Profile N`). Chromium expects that content under
      // the new user-data-dir's `Default` directory.
      Self::copy_directory_recursive(&source_path, &new_default_profile_dir)?;
      Self::sanitize_imported_profile(&new_default_profile_dir)?;

      let now = crate::proxy_manager::now_secs();
      let profile = BrowserProfile {
        id: profile_id,
        name: new_profile_name.trim().to_string(),
        browser: TARGET_KERNEL.to_string(),
        version,
        proxy_id,
        vpn_id: None,
        launch_hook: None,
        process_id: None,
        last_launch: None,
        release_type: "stable".to_string(),
        wayfern_config: None,
        persona: Some(persona),
        group_id: None,
        tags: Vec::new(),
        note: None,
        window_color: Some(crate::wayfern_manager::derive_profile_color(&profile_id)),
        sync_mode: SyncMode::Disabled,
        encryption_salt: None,
        last_sync: None,
        host_os: Some(get_host_os()),
        ephemeral: false,
        extension_group_id: None,
        proxy_bypass_rules: Vec::new(),
        created_by_id: None,
        created_by_email: None,
        dns_blocklist: None,
        password_protected: false,
        created_at: Some(now),
        updated_at: Some(now),
      };

      self.profile_manager.save_profile(&profile)?;
      Ok(())
    })();

    if let Err(error) = import_result {
      let _ = fs::remove_dir_all(&new_profile_uuid_dir);
      return Err(error);
    }

    let _ = crate::events::emit_empty("profiles-changed");
    log::info!(
      "Successfully imported {} profile '{}' into {} {} from '{}'",
      browser_type,
      new_profile_name.trim(),
      TARGET_KERNEL,
      AUDITED_KERNEL_VERSION,
      source_path.display()
    );
    Ok(())
  }

  fn get_audited_kernel_version(&self) -> Result<String, Box<dyn std::error::Error>> {
    let manifest = KernelManifest::embedded()?;
    let asset = manifest
      .find(TARGET_KERNEL, AUDITED_KERNEL_VERSION, current_platform_id())
      .ok_or_else(|| serde_json::json!({ "code": "KERNEL_NOT_INSTALLED" }).to_string())?;
    let registry = InstallRegistryFile::load();
    let installed = registry
      .find(TARGET_KERNEL, AUDITED_KERNEL_VERSION)
      .filter(|entry| {
        entry.platform == asset.platform
          && entry.sha256.eq_ignore_ascii_case(&asset.sha256)
          && entry.source_status == asset.source_status
          && Path::new(&entry.executable).is_file()
      })
      .ok_or_else(|| serde_json::json!({ "code": "KERNEL_NOT_INSTALLED" }).to_string())?;
    if !Path::new(&installed.install_path).is_dir() {
      return Err(
        serde_json::json!({ "code": "KERNEL_NOT_INSTALLED" })
          .to_string()
          .into(),
      );
    }
    Ok(asset.version.clone())
  }

  fn validate_source_profile(
    source: &Path,
    browser_type: &str,
  ) -> Result<PathBuf, Box<dyn std::error::Error>> {
    map_browser_type(browser_type)?;
    let metadata = fs::symlink_metadata(source)
      .map_err(|_| serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" }).to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
      return Err(
        serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" })
          .to_string()
          .into(),
      );
    }
    let canonical = fs::canonicalize(source)?;
    let preferences = canonical.join("Preferences");
    let preferences_meta = fs::symlink_metadata(&preferences)
      .map_err(|_| serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" }).to_string())?;
    if !preferences_meta.is_file()
      || preferences_meta.file_type().is_symlink()
      || preferences_meta.len() == 0
      || preferences_meta.len() > MAX_PREFERENCES_BYTES
    {
      return Err(
        serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" })
          .to_string()
          .into(),
      );
    }
    // A syntactically valid Preferences object is the minimum marker for a
    // Chrome/Edge/Chromium profile directory. We never execute imported data.
    let preferences_json: serde_json::Value = serde_json::from_reader(fs::File::open(preferences)?)
      .map_err(|_| serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" }).to_string())?;
    if !preferences_json.is_object() {
      return Err(
        serde_json::json!({ "code": "IMPORT_SOURCE_INVALID" })
          .to_string()
          .into(),
      );
    }
    Ok(canonical)
  }

  fn sanitize_imported_profile(profile_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Imported extensions and background workers are executable content. They
    // are deliberately removed; extensions must be re-added through the
    // audited extension manager, where hashes and requested permissions are
    // visible before assignment.
    const REMOVE_DIRS: &[&str] = &[
      "Extensions",
      "Extension Rules",
      "Extension Scripts",
      "Local Extension Settings",
      "Managed Extension Settings",
      "Sync Extension Settings",
      "Service Worker",
      "Background Sync",
      "Push Messaging",
      "Sessions",
      "Cache",
      "Code Cache",
      "GPUCache",
      "DawnCache",
      "GrShaderCache",
      "ShaderCache",
    ];
    const REMOVE_FILES: &[&str] = &[
      "Secure Preferences",
      "Current Session",
      "Current Tabs",
      "Last Session",
      "Last Tabs",
      "DevToolsActivePort",
      "LOCK",
    ];
    for relative in REMOVE_DIRS {
      let path = profile_dir.join(relative);
      if path.is_dir() {
        fs::remove_dir_all(path)?;
      }
    }
    for relative in REMOVE_FILES {
      let path = profile_dir.join(relative);
      if path.is_file() {
        fs::remove_file(path)?;
      }
    }

    let preferences_path = profile_dir.join("Preferences");
    let mut preferences: serde_json::Value =
      serde_json::from_reader(fs::File::open(&preferences_path)?)?;
    let root = preferences
      .as_object_mut()
      .ok_or("Preferences root is not an object")?;
    root.remove("extensions");
    root.remove("proxy");
    root.remove("session");
    root.remove("background_mode");
    if let Some(browser) = root
      .get_mut("browser")
      .and_then(|value| value.as_object_mut())
    {
      browser.remove("enabled_labs_experiments");
    }
    if let Some(profile) = root
      .get_mut("profile")
      .and_then(|value| value.as_object_mut())
    {
      profile.remove("content_settings");
      profile.insert(
        "exit_type".to_string(),
        serde_json::Value::String("Normal".to_string()),
      );
      profile.insert("exited_cleanly".to_string(), serde_json::Value::Bool(true));
    }
    if let Some(download) = root
      .get_mut("download")
      .and_then(|value| value.as_object_mut())
    {
      download.remove("default_directory");
      download.remove("savefile_default_directory");
    }

    let temporary = preferences_path.with_extension("json.importing");
    let serialized = serde_json::to_vec(&preferences)?;
    fs::write(&temporary, serialized)?;
    crate::kernel::install_registry::replace_file(&temporary, &preferences_path)?;
    Ok(())
  }

  pub fn copy_directory_recursive(
    source: &Path,
    destination: &Path,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let source = fs::canonicalize(source)?;
    let source_meta = fs::symlink_metadata(&source)?;
    if !source_meta.is_dir() || source_meta.file_type().is_symlink() {
      return Err("copy source must be a real directory".into());
    }

    let destination_parent = destination
      .parent()
      .ok_or("copy destination must have a parent")?;
    create_dir_all(destination_parent)?;
    let destination_abs = fs::canonicalize(destination_parent)?.join(
      destination
        .file_name()
        .ok_or("copy destination must have a file name")?,
    );
    if destination_abs.starts_with(&source) {
      return Err("copy destination must not be inside the source directory".into());
    }

    let mut entry_count = 0usize;
    let mut byte_count = 0u64;
    Self::copy_directory_bounded(&source, &destination_abs, &mut entry_count, &mut byte_count)
  }

  fn copy_directory_bounded(
    source: &Path,
    destination: &Path,
    entry_count: &mut usize,
    byte_count: &mut u64,
  ) -> Result<(), Box<dyn std::error::Error>> {
    create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
      let entry = entry?;
      *entry_count += 1;
      if *entry_count > MAX_IMPORT_ENTRIES {
        return Err("profile copy exceeds the entry safety limit".into());
      }

      let source_path = entry.path();
      let metadata = fs::symlink_metadata(&source_path)?;
      if metadata.file_type().is_symlink() {
        return Err(
          format!(
            "profile contains a symbolic link: {}",
            source_path.display()
          )
          .into(),
        );
      }
      let dest_path = destination.join(entry.file_name());
      if metadata.is_dir() {
        Self::copy_directory_bounded(&source_path, &dest_path, entry_count, byte_count)?;
      } else if metadata.is_file() {
        *byte_count = byte_count
          .checked_add(metadata.len())
          .ok_or("profile copy size overflow")?;
        if *byte_count > MAX_IMPORT_BYTES {
          return Err("profile copy exceeds the 20 GiB safety limit".into());
        }
        fs::copy(&source_path, &dest_path)?;
      } else {
        return Err(format!("profile contains a special file: {}", source_path.display()).into());
      }
    }
    Ok(())
  }
}

#[tauri::command]
pub async fn detect_existing_profiles() -> Result<Vec<DetectedProfile>, String> {
  let importer = ProfileImporter::instance();
  importer
    .detect_existing_profiles()
    .map_err(|e| format!("Failed to detect existing profiles: {e}"))
}

#[tauri::command]
pub async fn import_browser_profile(
  source_path: String,
  browser_type: String,
  new_profile_name: String,
  proxy_id: Option<String>,
) -> Result<(), String> {
  let importer = ProfileImporter::instance();
  importer
    .import_profile(&source_path, &browser_type, &new_profile_name, proxy_id)
    .await
    .map_err(|error| {
      let message = error.to_string();
      if message.starts_with('{') {
        message
      } else {
        serde_json::json!({
          "code": "IMPORT_FAILED",
          "params": { "detail": message }
        })
        .to_string()
      }
    })
}

lazy_static::lazy_static! {
  static ref PROFILE_IMPORTER: ProfileImporter = ProfileImporter::new();
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn create_test_profile_importer() -> (ProfileImporter, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let importer = ProfileImporter::new();
    (importer, temp_dir)
  }

  #[test]
  fn test_profile_importer_creation() {
    let (_importer, _temp_dir) = create_test_profile_importer();
  }

  #[test]
  fn test_get_browser_display_name() {
    let (importer, _temp_dir) = create_test_profile_importer();

    assert_eq!(importer.get_browser_display_name("chromium"), "Chromium");
    assert_eq!(importer.get_browser_display_name("chrome"), "Google Chrome");
    assert_eq!(importer.get_browser_display_name("edge"), "Microsoft Edge");
    assert_eq!(
      importer.get_browser_display_name("unknown"),
      "Unknown Browser"
    );
  }

  #[test]
  fn test_map_browser_type() {
    assert_eq!(map_browser_type("chrome").unwrap(), TARGET_KERNEL);
    assert_eq!(map_browser_type("edge").unwrap(), TARGET_KERNEL);
    assert_eq!(map_browser_type("chromium").unwrap(), TARGET_KERNEL);
    assert!(map_browser_type("brave").is_err());
    assert!(map_browser_type("wayfern").is_err());
    assert!(map_browser_type("something_else").is_err());
  }

  #[test]
  fn test_detect_existing_profiles_no_panic() {
    let (importer, _temp_dir) = create_test_profile_importer();

    let result = importer.detect_existing_profiles();
    assert!(result.is_ok(), "detect_existing_profiles should not fail");
    let _profiles = result.unwrap();
  }

  #[test]
  fn test_scan_chrome_profiles_dir_nonexistent() {
    let (importer, temp_dir) = create_test_profile_importer();

    let nonexistent_dir = temp_dir.path().join("nonexistent");
    let result = importer.scan_chrome_profiles_dir(&nonexistent_dir, "chromium");

    assert!(
      result.is_ok(),
      "Should handle nonexistent directory gracefully"
    );
    let profiles = result.unwrap();
    assert!(
      profiles.is_empty(),
      "Should return empty vector for nonexistent directory"
    );
  }

  #[test]
  fn test_copy_directory_recursive() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    let source_dir = temp_dir.path().join("source");
    let source_subdir = source_dir.join("subdir");
    fs::create_dir_all(&source_subdir).expect("Should create source directories");

    let source_file1 = source_dir.join("file1.txt");
    let source_file2 = source_subdir.join("file2.txt");
    fs::write(&source_file1, "content1").expect("Should create file1");
    fs::write(&source_file2, "content2").expect("Should create file2");

    let dest_dir = temp_dir.path().join("dest");

    let result = ProfileImporter::copy_directory_recursive(&source_dir, &dest_dir);
    assert!(result.is_ok(), "Should copy directory successfully");

    let dest_file1 = dest_dir.join("file1.txt");
    let dest_file2 = dest_dir.join("subdir").join("file2.txt");

    assert!(dest_file1.exists(), "file1.txt should be copied");
    assert!(dest_file2.exists(), "file2.txt should be copied");

    let content1 = fs::read_to_string(&dest_file1).expect("Should read file1");
    let content2 = fs::read_to_string(&dest_file2).expect("Should read file2");

    assert_eq!(content1, "content1", "file1 content should match");
    assert_eq!(content2, "content2", "file2 content should match");
  }

  #[test]
  fn test_validate_source_requires_valid_preferences_json() {
    let temp_dir = TempDir::new().unwrap();
    let source = temp_dir.path().join("Default");
    fs::create_dir_all(&source).unwrap();
    assert!(ProfileImporter::validate_source_profile(&source, "chrome").is_err());

    fs::write(source.join("Preferences"), "not-json").unwrap();
    assert!(ProfileImporter::validate_source_profile(&source, "chrome").is_err());

    fs::write(source.join("Preferences"), "{}").unwrap();
    assert!(ProfileImporter::validate_source_profile(&source, "chrome").is_ok());
    assert!(ProfileImporter::validate_source_profile(&source, "brave").is_err());
  }

  #[test]
  fn test_sanitize_imported_profile_removes_executable_state() {
    let temp_dir = TempDir::new().unwrap();
    let profile = temp_dir.path().join("profile");
    fs::create_dir_all(profile.join("Extensions/example")).unwrap();
    fs::create_dir_all(profile.join("Service Worker/ScriptCache")).unwrap();
    fs::write(profile.join("Secure Preferences"), "{}").unwrap();
    fs::write(
      profile.join("Preferences"),
      r#"{
        "extensions":{"settings":{"abc":{}}},
        "session":{"restore_on_startup":1},
        "proxy":{"mode":"fixed_servers"},
        "profile":{"content_settings":{"exceptions":{}},"exit_type":"Crashed"},
        "download":{"default_directory":"C:\\unsafe"}
      }"#,
    )
    .unwrap();

    ProfileImporter::sanitize_imported_profile(&profile).unwrap();
    assert!(!profile.join("Extensions").exists());
    assert!(!profile.join("Service Worker").exists());
    assert!(!profile.join("Secure Preferences").exists());
    let sanitized: serde_json::Value =
      serde_json::from_slice(&fs::read(profile.join("Preferences")).unwrap()).unwrap();
    assert!(sanitized.get("extensions").is_none());
    assert!(sanitized.get("session").is_none());
    assert!(sanitized.get("proxy").is_none());
    assert_eq!(sanitized["profile"]["exit_type"], "Normal");
    assert_eq!(sanitized["profile"]["exited_cleanly"], true);
    assert!(sanitized["profile"].get("content_settings").is_none());
    assert!(sanitized["download"].get("default_directory").is_none());
  }
}
