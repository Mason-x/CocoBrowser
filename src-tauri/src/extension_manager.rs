use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::events;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extension {
  pub id: String,
  pub name: String,
  pub file_name: String,
  pub file_type: String,
  pub browser_compatibility: Vec<String>,
  pub created_at: u64,
  pub updated_at: u64,
  #[serde(default)]
  pub sync_enabled: bool,
  #[serde(default)]
  pub last_sync: Option<u64>,
  #[serde(default)]
  pub version: Option<String>,
  #[serde(default)]
  pub description: Option<String>,
  #[serde(default)]
  pub author: Option<String>,
  #[serde(default)]
  pub homepage_url: Option<String>,
  #[serde(default)]
  pub content_sha256: String,
  #[serde(default)]
  pub manifest_version: Option<u64>,
  #[serde(default)]
  pub permissions: Vec<String>,
  #[serde(default)]
  pub host_permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionGroup {
  pub id: String,
  pub name: String,
  pub extension_ids: Vec<String>,
  pub created_at: u64,
  pub updated_at: u64,
  #[serde(default)]
  pub sync_enabled: bool,
  #[serde(default)]
  pub last_sync: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExtensionGroupsData {
  groups: Vec<ExtensionGroup>,
}

fn now_secs() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

fn extensions_base_dir() -> PathBuf {
  crate::app_dirs::extensions_dir()
}

fn extension_groups_file() -> PathBuf {
  crate::app_dirs::data_subdir().join("extension_groups.json")
}

fn determine_browser_compatibility(file_type: &str) -> Vec<String> {
  match file_type {
    "crx" | "zip" => vec!["chromium".to_string()],
    _ => vec![],
  }
}

fn get_file_type(file_name: &str) -> Option<String> {
  let ext = file_name.rsplit('.').next()?.to_lowercase();
  match ext.as_str() {
    "crx" | "zip" => Some(ext),
    _ => None,
  }
}

const MAX_EXTENSION_ARCHIVE_BYTES: usize = 100 * 1024 * 1024;
const MAX_EXTENSION_FILES: usize = 10_000;
const MAX_EXTENSION_UNPACKED_BYTES: u64 = 500 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ICON_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Default)]
struct ManifestMetadata {
  name: Option<String>,
  version: Option<String>,
  description: Option<String>,
  author: Option<String>,
  homepage_url: Option<String>,
  manifest_version: Option<u64>,
  permissions: Vec<String>,
  host_permissions: Vec<String>,
}

fn validate_archive_file_name(file_name: &str) -> Result<(), String> {
  if file_name.is_empty()
    || file_name.len() > 255
    || Path::new(file_name)
      .file_name()
      .and_then(|value| value.to_str())
      != Some(file_name)
  {
    return Err("Extension file name must be a plain base name".to_string());
  }
  Ok(())
}

fn validate_extension_metadata(extension: &Extension) -> Result<(), String> {
  uuid::Uuid::parse_str(&extension.id)
    .map_err(|_| "Extension metadata contains an invalid id".to_string())?;
  validate_archive_file_name(&extension.file_name)?;
  let detected_type = get_file_type(&extension.file_name)
    .ok_or_else(|| "Extension metadata contains an unsupported file type".to_string())?;
  if detected_type != extension.file_type {
    return Err("Extension metadata file type does not match its file name".to_string());
  }
  if !extension.content_sha256.is_empty()
    && (extension.content_sha256.len() != 64
      || !extension
        .content_sha256
        .chars()
        .all(|character| character.is_ascii_hexdigit()))
  {
    return Err("Extension metadata contains an invalid SHA-256".to_string());
  }
  Ok(())
}

fn is_safe_archive_path(path: &Path) -> bool {
  !path.is_absolute()
    && path.components().all(|component| {
      !matches!(
        component,
        Component::ParentDir | Component::RootDir | Component::Prefix(_)
      )
    })
}

fn find_zip_start(data: &[u8]) -> usize {
  for i in 0..data.len().saturating_sub(3) {
    if data[i] == 0x50 && data[i + 1] == 0x4B && data[i + 2] == 0x03 && data[i + 3] == 0x04 {
      return i;
    }
  }
  0
}

fn extract_manifest_metadata(
  file_data: &[u8],
  file_type: &str,
) -> Result<ManifestMetadata, String> {
  if file_data.is_empty() || file_data.len() > MAX_EXTENSION_ARCHIVE_BYTES {
    return Err("Extension archive must be between 1 byte and 100 MiB".to_string());
  }
  let zip_start = if file_type == "crx" {
    let offset = find_zip_start(file_data);
    if offset == 0 && !file_data.starts_with(b"PK\x03\x04") {
      return Err("CRX archive does not contain a ZIP payload".to_string());
    }
    offset
  } else {
    0
  };

  let cursor = std::io::Cursor::new(&file_data[zip_start..]);
  let mut archive = zip::ZipArchive::new(cursor)
    .map_err(|e| format!("Extension is not a valid ZIP/CRX archive: {e}"))?;
  if archive.is_empty() || archive.len() > MAX_EXTENSION_FILES {
    return Err(format!(
      "Extension archive contains an invalid number of entries: {}",
      archive.len()
    ));
  }

  let mut seen_paths = HashSet::new();
  let mut unpacked_bytes = 0u64;
  for index in 0..archive.len() {
    let entry = archive
      .by_index(index)
      .map_err(|e| format!("Unable to inspect extension entry {index}: {e}"))?;
    let enclosed = entry
      .enclosed_name()
      .ok_or_else(|| format!("Unsafe extension archive path: {}", entry.name()))?;
    if !is_safe_archive_path(&enclosed) || entry.name().contains(':') {
      return Err(format!("Unsafe extension archive path: {}", entry.name()));
    }
    let normalized = enclosed.to_string_lossy().replace('\\', "/");
    if !seen_paths.insert(normalized.clone()) {
      return Err(format!("Duplicate extension archive path: {normalized}"));
    }
    if entry
      .unix_mode()
      .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
      return Err(format!(
        "Extension archive contains a symlink: {normalized}"
      ));
    }
    unpacked_bytes = unpacked_bytes
      .checked_add(entry.size())
      .ok_or_else(|| "Extension unpacked size overflow".to_string())?;
    if unpacked_bytes > MAX_EXTENSION_UNPACKED_BYTES {
      return Err("Extension unpacked size exceeds 500 MiB".to_string());
    }
  }

  let mut manifest_file = archive
    .by_name("manifest.json")
    .map_err(|_| "Extension archive must contain manifest.json at its root".to_string())?;
  if manifest_file.size() == 0 || manifest_file.size() > MAX_MANIFEST_BYTES {
    return Err("Extension manifest must be between 1 byte and 1 MiB".to_string());
  }
  let mut manifest_content = String::new();
  std::io::Read::read_to_string(&mut manifest_file, &mut manifest_content)
    .map_err(|e| format!("Extension manifest is not valid UTF-8: {e}"))?;
  drop(manifest_file);
  let manifest: serde_json::Value = serde_json::from_str(&manifest_content)
    .map_err(|e| format!("Extension manifest is invalid JSON: {e}"))?;
  let object = manifest
    .as_object()
    .ok_or_else(|| "Extension manifest must be a JSON object".to_string())?;

  let name = object
    .get("name")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
  let version = object
    .get("version")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
  if name.as_deref().is_none_or(|value| value.trim().is_empty())
    || version
      .as_deref()
      .is_none_or(|value| value.trim().is_empty())
  {
    return Err("Extension manifest requires non-empty name and version fields".to_string());
  }
  let manifest_version = object.get("manifest_version").and_then(|v| v.as_u64());
  if !matches!(manifest_version, Some(2 | 3)) {
    return Err("Only Chromium manifest version 2 or 3 extensions are supported".to_string());
  }
  let description = object
    .get("description")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
  let author = object
    .get("author")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
  let homepage_url = object
    .get("homepage_url")
    .or_else(|| object.get("homepage"))
    .and_then(|v| v.as_str())
    .and_then(|value| {
      url::Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.to_string())
    });
  let parse_string_array = |field: &str| -> Result<Vec<String>, String> {
    let Some(value) = object.get(field) else {
      return Ok(Vec::new());
    };
    let array = value
      .as_array()
      .ok_or_else(|| format!("Extension manifest field '{field}' must be an array"))?;
    if array.len() > 512 {
      return Err(format!("Extension manifest field '{field}' is too large"));
    }
    array
      .iter()
      .map(|value| {
        value
          .as_str()
          .map(ToString::to_string)
          .ok_or_else(|| format!("Extension manifest field '{field}' must contain strings"))
      })
      .collect()
  };
  let mut permissions = parse_string_array("permissions")?;
  permissions.extend(parse_string_array("optional_permissions")?);
  let mut host_permissions = parse_string_array("host_permissions")?;
  host_permissions.extend(parse_string_array("optional_host_permissions")?);
  let permission_hosts: Vec<String> = permissions
    .iter()
    .filter(|permission| permission.contains("://") || permission.as_str() == "<all_urls>")
    .cloned()
    .collect();
  permissions
    .retain(|permission| !permission.contains("://") && permission.as_str() != "<all_urls>");
  host_permissions.extend(permission_hosts);
  if let Some(content_scripts) = object
    .get("content_scripts")
    .and_then(|value| value.as_array())
  {
    for script in content_scripts {
      if let Some(matches) = script.get("matches").and_then(|value| value.as_array()) {
        host_permissions.extend(
          matches
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string)),
        );
      }
    }
  }
  permissions.sort();
  permissions.dedup();
  host_permissions.sort();
  host_permissions.dedup();

  Ok(ManifestMetadata {
    name,
    version,
    description,
    author,
    homepage_url,
    manifest_version,
    permissions,
    host_permissions,
  })
}

fn extract_icon_from_archive(file_data: &[u8], file_type: &str) -> Option<(Vec<u8>, String)> {
  let zip_start = if file_type == "crx" {
    find_zip_start(file_data)
  } else {
    0
  };

  let cursor = std::io::Cursor::new(&file_data[zip_start..]);
  let mut archive = match zip::ZipArchive::new(cursor) {
    Ok(a) => a,
    Err(_) => return None,
  };

  let icon_path = {
    let manifest_content = if let Ok(mut file) = archive.by_name("manifest.json") {
      let mut contents = String::new();
      if std::io::Read::read_to_string(&mut file, &mut contents).is_ok() {
        Some(contents)
      } else {
        None
      }
    } else {
      None
    };

    let manifest_content = manifest_content?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content).ok()?;

    let mut best_path: Option<String> = None;
    let mut best_size: u32 = 0;

    if let Some(icons) = manifest.get("icons").and_then(|v| v.as_object()) {
      for (size_str, path_val) in icons {
        if let (Ok(size), Some(path)) = (size_str.parse::<u32>(), path_val.as_str()) {
          if size > best_size {
            best_size = size;
            best_path = Some(path.to_string());
          }
        }
      }
    }

    if best_path.is_none() {
      for key in &["action", "browser_action"] {
        if let Some(action) = manifest.get(*key) {
          if let Some(icon) = action.get("default_icon") {
            if let Some(path) = icon.as_str() {
              best_path = Some(path.to_string());
            } else if let Some(icons) = icon.as_object() {
              for (size_str, path_val) in icons {
                if let (Ok(size), Some(path)) = (size_str.parse::<u32>(), path_val.as_str()) {
                  if size > best_size {
                    best_size = size;
                    best_path = Some(path.to_string());
                  }
                }
              }
            }
          }
        }
      }
    }

    best_path
  };

  let icon_path = icon_path?;

  let clean_path = icon_path.trim_start_matches('/');
  let mut file = archive.by_name(clean_path).ok()?;
  if file.size() > MAX_ICON_BYTES {
    return None;
  }
  let mut data = Vec::new();
  std::io::Read::read_to_end(&mut file, &mut data).ok()?;

  let ext = clean_path
    .rsplit('.')
    .next()
    .unwrap_or("png")
    .to_lowercase();

  if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
    return None;
  }
  Some((data, ext))
}

pub struct ExtensionManager;

impl ExtensionManager {
  pub fn new() -> Self {
    Self
  }

  fn get_extension_dir(&self, ext_id: &str) -> PathBuf {
    let safe_id = uuid::Uuid::parse_str(ext_id)
      .map(|id| id.to_string())
      .unwrap_or_else(|_| "__invalid_extension_id__".to_string());
    extensions_base_dir().join(safe_id)
  }

  fn get_metadata_path(&self, ext_id: &str) -> PathBuf {
    self.get_extension_dir(ext_id).join("metadata.json")
  }

  fn get_file_dir(&self, ext_id: &str) -> PathBuf {
    self.get_extension_dir(ext_id).join("file")
  }

  pub fn get_file_dir_public(&self, ext_id: &str) -> PathBuf {
    self.get_file_dir(ext_id)
  }

  // Extension CRUD

  pub fn add_extension(
    &self,
    name: String,
    file_name: String,
    file_data: Vec<u8>,
  ) -> Result<Extension, Box<dyn std::error::Error>> {
    validate_archive_file_name(&file_name)?;
    let file_type =
      get_file_type(&file_name).ok_or_else(|| format!("Unsupported file type: {file_name}"))?;

    let browser_compatibility = determine_browser_compatibility(&file_type);
    if browser_compatibility.is_empty() {
      return Err(format!("Unsupported file type: {file_name}").into());
    }
    let now = now_secs();

    let manifest = extract_manifest_metadata(&file_data, &file_type)?;
    let content_sha256 = crate::kernel::downloader::sha256_hex(&file_data);

    // An empty/whitespace-only manifest name counts as absent so the
    // user-provided name still applies.
    let final_name = match manifest.name.clone() {
      Some(n) if !n.trim().is_empty() => n,
      _ => name,
    };

    if final_name.trim().is_empty() {
      return Err(
        serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" })
          .to_string()
          .into(),
      );
    }

    let ext = Extension {
      id: uuid::Uuid::new_v4().to_string(),
      name: final_name,
      file_name: file_name.clone(),
      file_type,
      browser_compatibility,
      created_at: now,
      updated_at: now,
      sync_enabled: crate::sync::is_sync_configured(),
      last_sync: None,
      version: manifest.version,
      description: manifest.description,
      author: manifest.author,
      homepage_url: manifest.homepage_url,
      content_sha256,
      manifest_version: manifest.manifest_version,
      permissions: manifest.permissions,
      host_permissions: manifest.host_permissions,
    };

    let extension_dir = self.get_extension_dir(&ext.id);
    let store_result = (|| -> Result<(), Box<dyn std::error::Error>> {
      let file_dir = self.get_file_dir(&ext.id);
      fs::create_dir_all(&file_dir)?;
      fs::write(file_dir.join(&file_name), &file_data)?;

      if let Some((icon_data, icon_ext)) = extract_icon_from_archive(&file_data, &ext.file_type) {
        let icon_path = extension_dir.join(format!("icon.{icon_ext}"));
        fs::write(icon_path, icon_data)?;
      }

      let metadata_path = self.get_metadata_path(&ext.id);
      let json = serde_json::to_string_pretty(&ext)?;
      fs::write(metadata_path, json)?;
      Ok(())
    })();
    if let Err(error) = store_result {
      let _ = fs::remove_dir_all(&extension_dir);
      return Err(error);
    }

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if ext.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let id = ext.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_sync(id).await;
        });
      }
    }

    Ok(ext)
  }

  pub fn get_extension(&self, id: &str) -> Result<Extension, Box<dyn std::error::Error>> {
    let metadata_path = self.get_metadata_path(id);
    if !metadata_path.exists() {
      return Err(format!("Extension with id '{id}' not found").into());
    }
    let content = fs::read_to_string(metadata_path)?;
    let ext: Extension = serde_json::from_str(&content)?;
    validate_extension_metadata(&ext)?;
    if ext.id != id {
      return Err("Extension metadata id does not match its directory".into());
    }
    Ok(ext)
  }

  pub fn list_extensions(&self) -> Result<Vec<Extension>, Box<dyn std::error::Error>> {
    let base = extensions_base_dir();
    if !base.exists() {
      return Ok(Vec::new());
    }

    let mut extensions = Vec::new();
    for entry in fs::read_dir(base)? {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        let metadata_path = entry.path().join("metadata.json");
        if metadata_path.exists() {
          let content = fs::read_to_string(&metadata_path)?;
          if let Ok(ext) = serde_json::from_str::<Extension>(&content) {
            if validate_extension_metadata(&ext).is_ok()
              && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&ext.id)
            {
              extensions.push(ext);
            } else {
              log::warn!(
                "Ignoring invalid extension metadata at {}",
                metadata_path.display()
              );
            }
          }
        }
      }
    }

    extensions.sort_by_key(|a| a.created_at);
    Ok(extensions)
  }

  pub fn update_extension(
    &self,
    id: &str,
    name: Option<String>,
    file_name: Option<String>,
    file_data: Option<Vec<u8>>,
  ) -> Result<Extension, Box<dyn std::error::Error>> {
    let mut ext = self.get_extension(id)?;

    let explicit_name_provided = name.is_some();
    if let Some(new_name) = name {
      if new_name.trim().is_empty() {
        return Err(
          serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" })
            .to_string()
            .into(),
        );
      }
      ext.name = new_name;
    }

    if let (Some(new_file_name), Some(data)) = (file_name, file_data) {
      validate_archive_file_name(&new_file_name)?;
      let new_file_type = get_file_type(&new_file_name)
        .ok_or_else(|| format!("Unsupported file type: {new_file_name}"))?;
      let manifest = extract_manifest_metadata(&data, &new_file_type)?;
      let content_sha256 = crate::kernel::downloader::sha256_hex(&data);

      // Validation and hashing complete before the existing archive is touched.
      let file_dir = self.get_file_dir(id);
      if file_dir.exists() {
        fs::remove_dir_all(&file_dir)?;
      }
      fs::create_dir_all(&file_dir)?;
      fs::write(file_dir.join(&new_file_name), &data)?;

      ext.file_name = new_file_name;
      ext.file_type = new_file_type.clone();
      ext.browser_compatibility = determine_browser_compatibility(&new_file_type);

      ext.version = manifest.version;
      ext.description = manifest.description;
      ext.author = manifest.author;
      ext.homepage_url = manifest.homepage_url;
      ext.manifest_version = manifest.manifest_version;
      ext.permissions = manifest.permissions;
      ext.host_permissions = manifest.host_permissions;
      ext.content_sha256 = content_sha256;
      if let Some(mn) = manifest.name {
        if !explicit_name_provided {
          ext.name = mn;
        }
      }

      if let Some((icon_data, icon_ext)) = extract_icon_from_archive(&data, &new_file_type) {
        let icon_path = self.get_extension_dir(id).join(format!("icon.{icon_ext}"));
        let _ = fs::write(icon_path, icon_data);
      }
    }

    ext.updated_at = now_secs();

    let metadata_path = self.get_metadata_path(id);
    let json = serde_json::to_string_pretty(&ext)?;
    fs::write(metadata_path, json)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if ext.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let eid = ext.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_sync(eid).await;
        });
      }
    }

    Ok(ext)
  }

  pub fn delete_extension(
    &self,
    app_handle: &tauri::AppHandle,
    id: &str,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let ext = self.get_extension(id)?;
    let ext_dir = self.get_extension_dir(id);
    if ext_dir.exists() {
      fs::remove_dir_all(&ext_dir)?;
    }

    // Remove from all groups
    let mut groups_data = self.load_groups_data()?;
    for group in &mut groups_data.groups {
      group.extension_ids.retain(|eid| eid != id);
    }
    self.save_groups_data(&groups_data)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if ext.sync_enabled {
      let ext_id = id.to_string();
      let app_handle_clone = app_handle.clone();
      tauri::async_runtime::spawn(async move {
        match crate::sync::SyncEngine::create_from_settings(&app_handle_clone).await {
          Ok(engine) => {
            if let Err(e) = engine.delete_extension(&ext_id).await {
              log::warn!("Failed to delete extension {} from sync: {}", ext_id, e);
            }
          }
          Err(e) => {
            log::debug!("Sync not configured, skipping remote deletion: {}", e);
          }
        }
      });
    }

    Ok(())
  }

  // Extension Group CRUD

  fn load_groups_data(&self) -> Result<ExtensionGroupsData, Box<dyn std::error::Error>> {
    let path = extension_groups_file();
    if !path.exists() {
      return Ok(ExtensionGroupsData { groups: Vec::new() });
    }
    let content = fs::read_to_string(path)?;
    let data: ExtensionGroupsData = serde_json::from_str(&content)?;
    Ok(data)
  }

  fn save_groups_data(&self, data: &ExtensionGroupsData) -> Result<(), Box<dyn std::error::Error>> {
    let path = extension_groups_file();
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)?;
    fs::write(path, json)?;
    Ok(())
  }

  pub fn create_group(&self, name: String) -> Result<ExtensionGroup, Box<dyn std::error::Error>> {
    if name.trim().is_empty() {
      return Err(
        serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" })
          .to_string()
          .into(),
      );
    }

    let mut data = self.load_groups_data()?;

    if data.groups.iter().any(|g| g.name == name) {
      return Err(format!("Extension group with name '{name}' already exists").into());
    }

    let now = now_secs();
    let group = ExtensionGroup {
      id: uuid::Uuid::new_v4().to_string(),
      name,
      extension_ids: Vec::new(),
      created_at: now,
      updated_at: now,
      sync_enabled: crate::sync::is_sync_configured(),
      last_sync: None,
    };

    data.groups.push(group.clone());
    self.save_groups_data(&data)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if group.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let id = group.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_group_sync(id).await;
        });
      }
    }

    Ok(group)
  }

  pub fn get_group(&self, id: &str) -> Result<ExtensionGroup, Box<dyn std::error::Error>> {
    let data = self.load_groups_data()?;
    data
      .groups
      .into_iter()
      .find(|g| g.id == id)
      .ok_or_else(|| format!("Extension group with id '{id}' not found").into())
  }

  pub fn list_groups(&self) -> Result<Vec<ExtensionGroup>, Box<dyn std::error::Error>> {
    let data = self.load_groups_data()?;
    Ok(data.groups)
  }

  pub fn update_group(
    &self,
    id: &str,
    name: Option<String>,
    extension_ids: Option<Vec<String>>,
  ) -> Result<ExtensionGroup, Box<dyn std::error::Error>> {
    if name.as_deref().is_some_and(|n| n.trim().is_empty()) {
      return Err(
        serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" })
          .to_string()
          .into(),
      );
    }

    let mut data = self.load_groups_data()?;

    if let Some(ref new_name) = name {
      if data
        .groups
        .iter()
        .any(|g| g.name == *new_name && g.id != id)
      {
        return Err(format!("Extension group with name '{new_name}' already exists").into());
      }
    }

    let group = data
      .groups
      .iter_mut()
      .find(|g| g.id == id)
      .ok_or_else(|| format!("Extension group with id '{id}' not found"))?;

    if let Some(new_name) = name {
      group.name = new_name;
    }
    if let Some(new_ids) = extension_ids {
      group.extension_ids = new_ids;
    }
    group.updated_at = now_secs();

    let updated = group.clone();
    self.save_groups_data(&data)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if updated.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let gid = updated.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_group_sync(gid).await;
        });
      }
    }

    Ok(updated)
  }

  pub fn delete_group(
    &self,
    app_handle: &tauri::AppHandle,
    id: &str,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = self.load_groups_data()?;

    let was_sync_enabled = data
      .groups
      .iter()
      .find(|g| g.id == id)
      .map(|g| g.sync_enabled)
      .unwrap_or(false);

    let initial_len = data.groups.len();
    data.groups.retain(|g| g.id != id);
    if data.groups.len() == initial_len {
      return Err(format!("Extension group with id '{id}' not found").into());
    }
    self.save_groups_data(&data)?;

    // Clear extension_group_id from profiles that used this group
    let profile_manager = crate::profile::ProfileManager::instance();
    if let Ok(profiles) = profile_manager.list_profiles() {
      for mut p in profiles {
        if p.extension_group_id.as_deref() == Some(id) {
          p.extension_group_id = None;
          let _ = profile_manager.save_profile(&p);
        }
      }
    }

    if was_sync_enabled {
      let group_id_owned = id.to_string();
      let app_handle_clone = app_handle.clone();
      tauri::async_runtime::spawn(async move {
        match crate::sync::SyncEngine::create_from_settings(&app_handle_clone).await {
          Ok(engine) => {
            if let Err(e) = engine.delete_extension_group(&group_id_owned).await {
              log::warn!(
                "Failed to delete extension group {} from sync: {}",
                group_id_owned,
                e
              );
            }
          }
          Err(e) => {
            log::debug!("Sync not configured, skipping remote deletion: {}", e);
          }
        }
      });
    }

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    Ok(())
  }

  pub fn add_extension_to_group(
    &self,
    group_id: &str,
    extension_id: &str,
  ) -> Result<ExtensionGroup, Box<dyn std::error::Error>> {
    // Verify extension exists
    let _ = self.get_extension(extension_id)?;

    let mut data = self.load_groups_data()?;
    let group = data
      .groups
      .iter_mut()
      .find(|g| g.id == group_id)
      .ok_or_else(|| format!("Extension group with id '{group_id}' not found"))?;

    if !group.extension_ids.contains(&extension_id.to_string()) {
      group.extension_ids.push(extension_id.to_string());
      group.updated_at = now_secs();
    }

    let updated = group.clone();
    self.save_groups_data(&data)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if updated.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let gid = updated.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_group_sync(gid).await;
        });
      }
    }

    Ok(updated)
  }

  pub fn remove_extension_from_group(
    &self,
    group_id: &str,
    extension_id: &str,
  ) -> Result<ExtensionGroup, Box<dyn std::error::Error>> {
    let mut data = self.load_groups_data()?;
    let group = data
      .groups
      .iter_mut()
      .find(|g| g.id == group_id)
      .ok_or_else(|| format!("Extension group with id '{group_id}' not found"))?;

    group.extension_ids.retain(|eid| eid != extension_id);
    group.updated_at = now_secs();

    let updated = group.clone();
    self.save_groups_data(&data)?;

    if let Err(e) = events::emit_empty("extensions-changed") {
      log::error!("Failed to emit extensions-changed event: {e}");
    }

    if updated.sync_enabled {
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        let gid = updated.id.clone();
        tauri::async_runtime::spawn(async move {
          scheduler.queue_extension_group_sync(gid).await;
        });
      }
    }

    Ok(updated)
  }

  // Sync helpers

  pub fn update_extension_internal(
    &self,
    ext: &Extension,
  ) -> Result<(), Box<dyn std::error::Error>> {
    validate_extension_metadata(ext)?;
    let metadata_path = self.get_metadata_path(&ext.id);
    if let Some(parent) = metadata_path.parent() {
      fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(ext)?;
    fs::write(metadata_path, json)?;
    Ok(())
  }

  pub fn upsert_extension_internal(
    &self,
    ext: &Extension,
  ) -> Result<(), Box<dyn std::error::Error>> {
    validate_extension_metadata(ext)?;
    self.update_extension_internal(ext)
  }

  pub fn delete_extension_internal(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ext_dir = self.get_extension_dir(id);
    if ext_dir.exists() {
      fs::remove_dir_all(&ext_dir)?;
    }
    // Remove from all groups
    let mut groups_data = self.load_groups_data()?;
    for group in &mut groups_data.groups {
      group.extension_ids.retain(|eid| eid != id);
    }
    self.save_groups_data(&groups_data)?;
    Ok(())
  }

  pub fn update_group_internal(
    &self,
    group: &ExtensionGroup,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = self.load_groups_data()?;
    if let Some(existing) = data.groups.iter_mut().find(|g| g.id == group.id) {
      existing.name = group.name.clone();
      existing.extension_ids = group.extension_ids.clone();
      existing.sync_enabled = group.sync_enabled;
      existing.last_sync = group.last_sync;
      existing.updated_at = group.updated_at;
      self.save_groups_data(&data)?;
    }
    Ok(())
  }

  pub fn upsert_group_internal(
    &self,
    group: &ExtensionGroup,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = self.load_groups_data()?;
    if let Some(existing) = data.groups.iter_mut().find(|g| g.id == group.id) {
      existing.name = group.name.clone();
      existing.extension_ids = group.extension_ids.clone();
      existing.sync_enabled = group.sync_enabled;
      existing.last_sync = group.last_sync;
      existing.updated_at = group.updated_at;
    } else {
      data.groups.push(group.clone());
    }
    self.save_groups_data(&data)?;
    Ok(())
  }

  pub fn delete_group_internal(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = self.load_groups_data()?;
    data.groups.retain(|g| g.id != id);
    self.save_groups_data(&data)?;
    Ok(())
  }

  // Compatibility validation

  pub fn validate_group_compatibility(
    &self,
    group_id: &str,
    browser: &str,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let group = self.get_group(group_id)?;
    let browser_type = match browser {
      "fingerprint-chromium" | "cloakbrowser-146" | "cloakbrowser-150" => "chromium",
      _ => return Err(format!("Extensions are not supported for browser '{browser}'").into()),
    };

    for ext_id in &group.extension_ids {
      let ext = self.get_extension(ext_id)?;
      if !ext
        .browser_compatibility
        .contains(&browser_type.to_string())
      {
        return Err(
          format!(
            "Extension '{}' ({}) is not compatible with {} browsers",
            ext.name, ext.file_type, browser_type
          )
          .into(),
        );
      }
    }

    Ok(())
  }

  // Launch-time installation

  pub fn install_extensions_for_profile(
    &self,
    profile: &crate::profile::BrowserProfile,
    _profile_data_path: &std::path::Path,
  ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let group_id = match &profile.extension_group_id {
      Some(id) => id,
      None => return Ok(Vec::new()),
    };

    let group = self.get_group(group_id)?;
    if group.extension_ids.is_empty() {
      return Ok(Vec::new());
    }

    if !crate::kernel::kinds::is_persona_kernel(&profile.browser) {
      return Ok(Vec::new());
    }

    let mut extension_paths = Vec::new();

    for ext_id in &group.extension_ids {
      if let Ok(ext) = self.get_extension(ext_id) {
        if !ext.browser_compatibility.contains(&"chromium".to_string()) {
          continue;
        }
        let src_file = self.get_file_dir(ext_id).join(&ext.file_name);
        if src_file.exists() {
          let archive_size = fs::metadata(&src_file)?.len();
          if archive_size == 0 || archive_size > MAX_EXTENSION_ARCHIVE_BYTES as u64 {
            return Err(
              format!("Extension '{}' exceeds the archive safety limit", ext.name).into(),
            );
          }
          let archive_data = fs::read(&src_file)?;
          extract_manifest_metadata(&archive_data, &ext.file_type)
            .map_err(|error| format!("Extension '{}' is invalid: {error}", ext.name))?;
          let actual_hash = crate::kernel::downloader::sha256_hex(&archive_data);
          if !ext.content_sha256.is_empty()
            && !actual_hash.eq_ignore_ascii_case(&ext.content_sha256)
          {
            return Err(
              format!(
                "Extension '{}' failed its SHA-256 integrity check",
                ext.name
              )
              .into(),
            );
          }
          let unpack_dir = self
            .get_extension_dir(ext_id)
            .join("unpacked")
            .join(&actual_hash[..16]);
          let marker = unpack_dir.join(".coco-sha256");
          let already_valid = unpack_dir.join("manifest.json").is_file()
            && fs::read_to_string(&marker)
              .is_ok_and(|stored| stored.trim().eq_ignore_ascii_case(&actual_hash));

          if !already_valid {
            let staging = self
              .get_extension_dir(ext_id)
              .join("unpacked")
              .join(format!(".staging-{}", uuid::Uuid::new_v4()));
            if let Err(error) = (|| -> Result<(), Box<dyn std::error::Error>> {
              Self::unpack_extension(&src_file, &staging)?;
              fs::write(staging.join(".coco-sha256"), &actual_hash)?;
              if unpack_dir.exists() {
                fs::remove_dir_all(&unpack_dir)?;
              }
              fs::rename(&staging, &unpack_dir)?;
              Ok(())
            })() {
              let _ = fs::remove_dir_all(&staging);
              return Err(format!("Failed to unpack extension '{}': {error}", ext.name).into());
            }
          }
          extension_paths.push(unpack_dir.to_string_lossy().to_string());
        }
      }
    }

    Ok(extension_paths)
  }

  fn unpack_extension(
    src: &std::path::Path,
    dest: &std::path::Path,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let archive_size = fs::metadata(src)?.len();
    if archive_size == 0 || archive_size > MAX_EXTENSION_ARCHIVE_BYTES as u64 {
      return Err("Extension archive must be between 1 byte and 100 MiB".into());
    }
    let data = fs::read(src)?;
    let mut archive = match zip::ZipArchive::new(std::io::Cursor::new(data.as_slice())) {
      Ok(a) => a,
      Err(e) => {
        // CRX files have a header before the ZIP data 鈥?try skipping the CRX header
        if let Some(zip_start) = Self::find_zip_start(&data) {
          zip::ZipArchive::new(std::io::Cursor::new(&data[zip_start..]))
            .map_err(|e2| format!("Failed to open CRX as zip after header skip: {e2}"))?
        } else {
          return Err(format!("Failed to open as zip: {e}").into());
        }
      }
    };
    if archive.is_empty() || archive.len() > MAX_EXTENSION_FILES {
      return Err("Extension archive entry count exceeds the safety limit".into());
    }
    fs::create_dir_all(dest)?;
    let mut seen_paths = HashSet::new();
    let mut unpacked_bytes = 0u64;
    for i in 0..archive.len() {
      let mut file = archive.by_index(i)?;
      let enclosed = file
        .enclosed_name()
        .ok_or_else(|| format!("Unsafe extension archive path: {}", file.name()))?;
      if !is_safe_archive_path(&enclosed) || file.name().contains(':') {
        return Err(format!("Unsafe extension archive path: {}", file.name()).into());
      }
      let normalized = enclosed.to_string_lossy().replace('\\', "/");
      if !seen_paths.insert(normalized.clone()) {
        return Err(format!("Duplicate extension archive path: {normalized}").into());
      }
      if file
        .unix_mode()
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
      {
        return Err(format!("Extension archive contains a symlink: {normalized}").into());
      }
      unpacked_bytes = unpacked_bytes
        .checked_add(file.size())
        .ok_or("Extension unpacked size overflow")?;
      if unpacked_bytes > MAX_EXTENSION_UNPACKED_BYTES {
        return Err("Extension unpacked size exceeds 500 MiB".into());
      }
      let out_path = dest.join(enclosed);

      if file.is_dir() {
        fs::create_dir_all(&out_path)?;
      } else {
        if let Some(parent) = out_path.parent() {
          fs::create_dir_all(parent)?;
        }
        let mut out_file = fs::File::create(&out_path)?;
        std::io::copy(&mut file, &mut out_file)?;
      }
    }

    if !dest.join("manifest.json").is_file() {
      return Err("Unpacked extension is missing manifest.json".into());
    }

    Ok(())
  }

  fn find_zip_start(data: &[u8]) -> Option<usize> {
    // ZIP local file header magic: PK\x03\x04
    let magic = [0x50, 0x4B, 0x03, 0x04];
    data.windows(4).position(|window| window == magic)
  }

  pub fn ensure_icons_extracted(&self) {
    let extensions = match self.list_extensions() {
      Ok(exts) => exts,
      Err(_) => return,
    };

    for ext in extensions {
      let ext_dir = self.get_extension_dir(&ext.id);
      let has_icon = ext_dir
        .read_dir()
        .map(|entries| {
          entries
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("icon."))
        })
        .unwrap_or(false);

      let file_dir = self.get_file_dir(&ext.id);
      let file_path = file_dir.join(&ext.file_name);
      let archive_is_bounded = fs::metadata(&file_path)
        .is_ok_and(|meta| meta.len() > 0 && meta.len() <= MAX_EXTENSION_ARCHIVE_BYTES as u64);
      if !archive_is_bounded {
        log::warn!(
          "Skipping invalid extension archive while backfilling '{}': size",
          ext.name
        );
        continue;
      }
      if let Ok(file_data) = fs::read(&file_path) {
        if !has_icon {
          if let Some((icon_data, icon_ext)) = extract_icon_from_archive(&file_data, &ext.file_type)
          {
            let icon_path = ext_dir.join(format!("icon.{icon_ext}"));
            let _ = fs::write(icon_path, icon_data);
          }
        }

        if let Ok(manifest) = extract_manifest_metadata(&file_data, &ext.file_type) {
          let mut updated_ext = ext.clone();
          updated_ext.version = manifest.version;
          updated_ext.description = manifest.description;
          updated_ext.author = manifest.author;
          updated_ext.homepage_url = manifest.homepage_url;
          updated_ext.manifest_version = manifest.manifest_version;
          updated_ext.permissions = manifest.permissions;
          updated_ext.host_permissions = manifest.host_permissions;
          updated_ext.content_sha256 = crate::kernel::downloader::sha256_hex(&file_data);
          let metadata_path = self.get_metadata_path(&ext.id);
          if let Ok(json) = serde_json::to_string_pretty(&updated_ext) {
            let _ = fs::write(metadata_path, json);
          }
        }
      }
    }
  }

  pub fn get_extension_icon(&self, ext_id: &str) -> Option<String> {
    let ext_dir = self.get_extension_dir(ext_id);
    let entries = ext_dir.read_dir().ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
      let name = entry.file_name().to_string_lossy().to_string();
      if name.starts_with("icon.") {
        let icon_path = entry.path();
        let data = fs::read(&icon_path).ok()?;
        let ext = name.rsplit('.').next().unwrap_or("png");
        let mime = match ext {
          "png" => "image/png",
          "jpg" | "jpeg" => "image/jpeg",
          "svg" => "image/svg+xml",
          "gif" => "image/gif",
          "webp" => "image/webp",
          _ => "image/png",
        };
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        return Some(format!("data:{};base64,{}", mime, b64));
      }
    }
    None
  }
}

// Global instance
lazy_static::lazy_static! {
  pub static ref EXTENSION_MANAGER: Mutex<ExtensionManager> = Mutex::new(ExtensionManager::new());
}

// Tauri commands

#[tauri::command]
pub async fn list_extensions() -> Result<Vec<Extension>, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .list_extensions()
    .map_err(|e| format!("Failed to list extensions: {e}"))
}

#[tauri::command]
pub fn get_extension_icon(extension_id: String) -> Option<String> {
  let manager = crate::extension_manager::ExtensionManager::new();
  manager.get_extension_icon(&extension_id)
}

#[tauri::command]
pub async fn add_extension(
  name: String,
  file_name: String,
  file_data: Vec<u8>,
) -> Result<Extension, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .add_extension(name, file_name, file_data)
    .map_err(extension_package_error)
}

#[tauri::command]
pub async fn update_extension(
  extension_id: String,
  name: Option<String>,
  file_name: Option<String>,
  file_data: Option<Vec<u8>>,
) -> Result<Extension, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .update_extension(&extension_id, name, file_name, file_data)
    .map_err(extension_package_error)
}

fn extension_package_error(error: impl std::fmt::Display) -> String {
  let message = error.to_string();
  if message.starts_with('{') {
    message
  } else {
    serde_json::json!({
      "code": "EXTENSION_PACKAGE_INVALID",
      "params": { "detail": message }
    })
    .to_string()
  }
}

#[tauri::command]
pub async fn delete_extension(
  app_handle: tauri::AppHandle,
  extension_id: String,
) -> Result<(), String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .delete_extension(&app_handle, &extension_id)
    .map_err(|e| format!("Failed to delete extension: {e}"))
}

#[tauri::command]
pub async fn list_extension_groups() -> Result<Vec<ExtensionGroup>, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .list_groups()
    .map_err(|e| format!("Failed to list extension groups: {e}"))
}

#[tauri::command]
pub async fn create_extension_group(name: String) -> Result<ExtensionGroup, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .create_group(name)
    .map_err(|e| crate::wrap_backend_error(e, "Failed to create extension group"))
}

#[tauri::command]
pub async fn update_extension_group(
  group_id: String,
  name: Option<String>,
  extension_ids: Option<Vec<String>>,
) -> Result<ExtensionGroup, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .update_group(&group_id, name, extension_ids)
    .map_err(|e| crate::wrap_backend_error(e, "Failed to update extension group"))
}

#[tauri::command]
pub async fn delete_extension_group(
  app_handle: tauri::AppHandle,
  group_id: String,
) -> Result<(), String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .delete_group(&app_handle, &group_id)
    .map_err(|e| format!("Failed to delete extension group: {e}"))
}

#[tauri::command]
pub async fn add_extension_to_group(
  group_id: String,
  extension_id: String,
) -> Result<ExtensionGroup, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .add_extension_to_group(&group_id, &extension_id)
    .map_err(|e| format!("Failed to add extension to group: {e}"))
}

#[tauri::command]
pub async fn remove_extension_from_group(
  group_id: String,
  extension_id: String,
) -> Result<ExtensionGroup, String> {
  let mgr = EXTENSION_MANAGER.lock().unwrap();
  mgr
    .remove_extension_from_group(&group_id, &extension_id)
    .map_err(|e| format!("Failed to remove extension from group: {e}"))
}

#[tauri::command]
pub async fn assign_extension_group_to_profile(
  profile_id: String,
  extension_group_id: Option<String>,
) -> Result<crate::profile::BrowserProfile, String> {
  // Validate compatibility if assigning a group
  if let Some(ref group_id) = extension_group_id {
    let profile_manager = crate::profile::ProfileManager::instance();
    let profiles = profile_manager
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles: {e}"))?;
    let profile = profiles
      .iter()
      .find(|p| p.id.to_string() == profile_id)
      .ok_or_else(|| format!("Profile '{profile_id}' not found"))?;

    let mgr = EXTENSION_MANAGER.lock().unwrap();
    mgr
      .validate_group_compatibility(group_id, &profile.browser)
      .map_err(|e| format!("{e}"))?;
  }

  let profile_manager = crate::profile::ProfileManager::instance();
  profile_manager
    .update_profile_extension_group(&profile_id, extension_group_id)
    .map_err(|e| format!("Failed to assign extension group: {e}"))
}

#[tauri::command]
pub async fn get_extension_group_for_profile(
  profile_id: String,
) -> Result<Option<ExtensionGroup>, String> {
  let profile_manager = crate::profile::ProfileManager::instance();
  let profiles = profile_manager
    .list_profiles()
    .map_err(|e| format!("Failed to list profiles: {e}"))?;
  let profile = profiles
    .iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| format!("Profile '{profile_id}' not found"))?;

  match &profile.extension_group_id {
    Some(group_id) => {
      let mgr = EXTENSION_MANAGER.lock().unwrap();
      match mgr.get_group(group_id) {
        Ok(group) => Ok(Some(group)),
        Err(_) => Ok(None),
      }
    }
    None => Ok(None),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use zip::write::SimpleFileOptions;

  fn valid_extension_zip(name: &str) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
      .start_file("manifest.json", SimpleFileOptions::default())
      .unwrap();
    write!(
      writer,
      "{{\"manifest_version\":3,\"name\":{},\"version\":\"1.0.0\",\"permissions\":[\"storage\"],\"host_permissions\":[\"https://example.com/*\"]}}",
      serde_json::to_string(name).unwrap()
    )
    .unwrap();
    writer.finish().unwrap().into_inner()
  }

  #[test]
  fn test_get_file_type() {
    assert_eq!(get_file_type("ext.crx"), Some("crx".to_string()));
    assert_eq!(get_file_type("ext.zip"), Some("zip".to_string()));
    assert_eq!(get_file_type("ublock.xpi"), None);
    assert_eq!(get_file_type("readme.txt"), None);
    assert_eq!(get_file_type("noext"), None);
  }

  #[test]
  fn test_determine_browser_compatibility() {
    assert_eq!(
      determine_browser_compatibility("crx"),
      vec!["chromium".to_string()]
    );
    assert_eq!(
      determine_browser_compatibility("zip"),
      vec!["chromium".to_string()]
    );
    assert_eq!(determine_browser_compatibility("xpi"), Vec::<String>::new());
  }

  #[test]
  fn test_extension_manager_crud() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());

    let mgr = ExtensionManager::new();

    // List empty
    let exts = mgr.list_extensions().unwrap();
    assert!(exts.is_empty());

    // Add
    let ext = mgr
      .add_extension(
        "Test Ext".to_string(),
        "test.zip".to_string(),
        valid_extension_zip("Test Ext"),
      )
      .unwrap();
    assert_eq!(ext.name, "Test Ext");
    assert_eq!(ext.file_type, "zip");
    assert_eq!(ext.browser_compatibility, vec!["chromium".to_string()]);
    assert_eq!(ext.content_sha256.len(), 64);
    assert_eq!(ext.manifest_version, Some(3));
    assert_eq!(ext.permissions, vec!["storage".to_string()]);

    // Get
    let fetched = mgr.get_extension(&ext.id).unwrap();
    assert_eq!(fetched.name, "Test Ext");

    // List
    let exts = mgr.list_extensions().unwrap();
    assert_eq!(exts.len(), 1);

    // Update name
    let updated = mgr
      .update_extension(&ext.id, Some("Updated".to_string()), None, None)
      .unwrap();
    assert_eq!(updated.name, "Updated");

    // Delete
    mgr.delete_extension_internal(&ext.id).unwrap();
    let exts = mgr.list_extensions().unwrap();
    assert!(exts.is_empty());
  }

  #[test]
  fn test_extension_group_crud() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());

    let mgr = ExtensionManager::new();

    // Create group
    let group = mgr.create_group("My Group".to_string()).unwrap();
    assert_eq!(group.name, "My Group");
    assert!(group.extension_ids.is_empty());

    // List groups
    let groups = mgr.list_groups().unwrap();
    assert_eq!(groups.len(), 1);

    // Add extension
    let ext = mgr
      .add_extension(
        "Test Ext".to_string(),
        "test.zip".to_string(),
        valid_extension_zip("Test Ext"),
      )
      .unwrap();

    // Add to group
    let updated = mgr.add_extension_to_group(&group.id, &ext.id).unwrap();
    assert_eq!(updated.extension_ids.len(), 1);

    // Remove from group
    let updated = mgr.remove_extension_from_group(&group.id, &ext.id).unwrap();
    assert!(updated.extension_ids.is_empty());

    // Duplicate name check
    let err = mgr.create_group("My Group".to_string());
    assert!(err.is_err());
  }

  #[test]
  fn test_validate_group_compatibility() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());

    let mgr = ExtensionManager::new();

    let chrome_ext = mgr
      .add_extension(
        "Chromium Ext".to_string(),
        "test.crx".to_string(),
        valid_extension_zip("Chromium Ext"),
      )
      .unwrap();
    let chrome_group = mgr.create_group("Chromium Group".to_string()).unwrap();
    mgr
      .add_extension_to_group(&chrome_group.id, &chrome_ext.id)
      .unwrap();

    assert!(mgr
      .validate_group_compatibility(&chrome_group.id, "fingerprint-chromium")
      .is_ok());
  }

  #[test]
  fn test_find_zip_start() {
    let data = vec![0x00, 0x00, 0x50, 0x4B, 0x03, 0x04, 0xFF];
    assert_eq!(ExtensionManager::find_zip_start(&data), Some(2));

    let data = vec![0x50, 0x4B, 0x03, 0x04, 0xFF];
    assert_eq!(ExtensionManager::find_zip_start(&data), Some(0));

    let data = vec![0x00, 0x00, 0x00];
    assert_eq!(ExtensionManager::find_zip_start(&data), None);
  }

  #[test]
  fn test_delete_extension_removes_from_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());

    let mgr = ExtensionManager::new();

    let ext = mgr
      .add_extension(
        "Test".to_string(),
        "test.zip".to_string(),
        valid_extension_zip("Test"),
      )
      .unwrap();

    let group = mgr.create_group("G1".to_string()).unwrap();
    mgr.add_extension_to_group(&group.id, &ext.id).unwrap();

    // Delete extension should remove from group
    mgr.delete_extension_internal(&ext.id).unwrap();

    let updated_group = mgr.get_group(&group.id).unwrap();
    assert!(updated_group.extension_ids.is_empty());
  }

  #[test]
  fn test_rejects_invalid_archive_and_unsafe_file_name() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());
    let mgr = ExtensionManager::new();
    assert!(mgr
      .add_extension("Bad".into(), "bad.zip".into(), vec![0, 1, 2, 3])
      .is_err());
    assert!(mgr
      .add_extension(
        "Bad".into(),
        "../bad.zip".into(),
        valid_extension_zip("Bad"),
      )
      .is_err());
  }
}
