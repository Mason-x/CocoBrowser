//! Local registry of installed kernels under `binaries/<id>/<version>/`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledKernel {
  pub id: String,
  pub version: String,
  pub platform: String,
  pub install_path: String,
  pub executable: String,
  pub sha256: String,
  pub source_status: String,
  pub installed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallRegistryFile {
  pub schema_version: u32,
  pub kernels: Vec<InstalledKernel>,
}

impl InstallRegistryFile {
  pub fn path() -> PathBuf {
    crate::app_dirs::data_subdir().join("installed_kernels.json")
  }

  pub fn load() -> Self {
    let path = Self::path();
    match fs::read_to_string(&path) {
      Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
      Err(_) => Self {
        schema_version: 1,
        kernels: vec![],
      },
    }
  }

  pub fn save(&self) -> Result<(), String> {
    let path = Self::path();
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).map_err(|e| format!("create registry dir: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
    fs::write(&tmp, json).map_err(|e| format!("write registry tmp: {e}"))?;
    replace_file(&tmp, &path)?;
    Ok(())
  }

  pub fn upsert(&mut self, entry: InstalledKernel) {
    if let Some(existing) = self
      .kernels
      .iter_mut()
      .find(|k| k.id == entry.id && k.version == entry.version && k.platform == entry.platform)
    {
      *existing = entry;
    } else {
      self.kernels.push(entry);
    }
  }

  pub fn find(&self, id: &str, version: &str) -> Option<&InstalledKernel> {
    self
      .kernels
      .iter()
      .find(|k| k.id == id && k.version == version)
  }

  pub fn list_for_id(&self, id: &str) -> Vec<&InstalledKernel> {
    self.kernels.iter().filter(|k| k.id == id).collect()
  }
}

#[cfg(windows)]
pub(crate) fn replace_file(tmp: &Path, destination: &Path) -> Result<(), String> {
  use std::os::windows::ffi::OsStrExt;
  use windows::core::PCWSTR;
  use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

  if !destination.exists() {
    return fs::rename(tmp, destination).map_err(|e| format!("install registry file: {e}"));
  }
  let destination_wide: Vec<u16> = destination
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();
  let tmp_wide: Vec<u16> = tmp
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();
  unsafe {
    ReplaceFileW(
      PCWSTR(destination_wide.as_ptr()),
      PCWSTR(tmp_wide.as_ptr()),
      PCWSTR::null(),
      REPLACE_FILE_FLAGS(0),
      None,
      None,
    )
  }
  .map_err(|e| format!("atomically replace registry: {e}"))
}

#[cfg(not(windows))]
pub(crate) fn replace_file(tmp: &Path, destination: &Path) -> Result<(), String> {
  fs::rename(tmp, destination).map_err(|e| format!("atomically replace registry: {e}"))
}

pub fn install_root(id: &str, version: &str) -> PathBuf {
  crate::app_dirs::binaries_dir().join(id).join(version)
}

pub fn now_secs() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0)
}

/// Scan an install root for an executable matching candidate relative paths.
pub fn find_executable(root: &Path, candidates: &[String]) -> Option<PathBuf> {
  for rel in candidates {
    let p = root.join(rel);
    if p.is_file() {
      return Some(p);
    }
  }
  // Fallback: shallow search for chrome.exe / chromium / wayfern names
  let names = [
    "chrome.exe",
    "chromium.exe",
    "chrome",
    "chromium",
    "wayfern.exe",
    "wayfern",
  ];
  for name in names {
    let direct = root.join(name);
    if direct.is_file() {
      return Some(direct);
    }
  }
  if let Ok(entries) = fs::read_dir(root) {
    for entry in entries.flatten() {
      let path = entry.path();
      if path.is_dir() {
        for name in names {
          let nested = path.join(name);
          if nested.is_file() {
            return Some(nested);
          }
        }
        // one more level (Chromium/Application/chrome.exe)
        if let Ok(sub) = fs::read_dir(&path) {
          for s in sub.flatten() {
            let sp = s.path();
            if sp.is_dir() {
              for name in names {
                let deep = sp.join(name);
                if deep.is_file() {
                  return Some(deep);
                }
              }
            }
          }
        }
      }
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn upsert_and_find() {
    let tmp = TempDir::new().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(tmp.path().to_path_buf());
    let mut reg = InstallRegistryFile {
      schema_version: 1,
      kernels: vec![],
    };
    reg.upsert(InstalledKernel {
      id: "fingerprint-chromium".into(),
      version: "148.0.7778.215".into(),
      platform: "windows-x64".into(),
      install_path: "/x".into(),
      executable: "/x/chrome.exe".into(),
      sha256: "ab".into(),
      source_status: "binary-source-delayed".into(),
      installed_at: 1,
    });
    reg.upsert(InstalledKernel {
      id: "fingerprint-chromium".into(),
      version: "148.0.7778.215".into(),
      platform: "windows-x64".into(),
      install_path: "/y".into(),
      executable: "/y/chrome.exe".into(),
      sha256: "cd".into(),
      source_status: "binary-source-delayed".into(),
      installed_at: 2,
    });
    assert_eq!(reg.kernels.len(), 1);
    assert_eq!(
      reg
        .find("fingerprint-chromium", "148.0.7778.215")
        .unwrap()
        .install_path,
      "/y"
    );
    reg.save().unwrap();
    reg.save().unwrap();
    let loaded = InstallRegistryFile::load();
    assert_eq!(loaded.kernels.len(), 1);
  }
}
