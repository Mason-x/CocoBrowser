//! Browser version comparison and on-disk version cache.
//!
//! This module performs no network I/O. It used to also fetch the upstream
//! engine's published version over HTTP; that endpoint belonged to the upstream
//! project's service and was removed with the engine.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionComponent {
  pub major: u32,
  pub minor: u32,
  pub patch: u32,
  pub build: u32,
}

impl VersionComponent {
  pub fn parse(version: &str) -> Self {
    let version = version.trim();
    let version = if version.starts_with('v') || version.starts_with('V') {
      &version[1..]
    } else {
      version
    };

    let numeric_part = Self::numeric_prefix(version);

    let parts: Vec<u32> = numeric_part
      .split('.')
      .filter_map(|part| part.parse().ok())
      .collect();

    VersionComponent {
      major: parts.first().copied().unwrap_or(0),
      minor: parts.get(1).copied().unwrap_or(0),
      patch: parts.get(2).copied().unwrap_or(0),
      build: parts.get(3).copied().unwrap_or(0),
    }
  }

  fn numeric_prefix(version: &str) -> String {
    let version = version.to_lowercase();
    for (i, ch) in version.char_indices() {
      if ch.is_alphabetic() && i > 0 {
        return version[..i].to_string();
      }
    }
    version
  }
}

impl PartialOrd for VersionComponent {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for VersionComponent {
  fn cmp(&self, other: &Self) -> std::cmp::Ordering {
    (self.major, self.minor, self.patch, self.build).cmp(&(
      other.major,
      other.minor,
      other.patch,
      other.build,
    ))
  }
}

pub fn sort_versions(versions: &mut [String]) {
  versions.sort_by(|a, b| {
    let version_a = VersionComponent::parse(a);
    let version_b = VersionComponent::parse(b);
    version_b.cmp(&version_a)
  });
}

pub fn compare_versions(version1: &str, version2: &str) -> std::cmp::Ordering {
  let version_a = VersionComponent::parse(version1);
  let version_b = VersionComponent::parse(version2);
  version_a.cmp(&version_b)
}

pub fn is_version_newer(version1: &str, version2: &str) -> bool {
  let version_a = VersionComponent::parse(version1);
  let version_b = VersionComponent::parse(version2);
  version_a > version_b
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BrowserRelease {
  pub version: String,
  pub date: String,
}

/// On-disk cache entry for a browser's version list.
#[derive(Debug, Serialize, Deserialize)]
struct CachedVersionData {
  releases: Vec<BrowserRelease>,
  timestamp: u64,
}

pub struct VersionCache;

impl VersionCache {
  pub fn new() -> Self {
    Self
  }

  pub fn instance() -> &'static VersionCache {
    &VERSION_CACHE
  }

  fn get_cache_dir() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let cache_dir = crate::app_dirs::cache_dir().join("version_cache");
    fs::create_dir_all(&cache_dir)?;
    Ok(cache_dir)
  }

  fn get_current_timestamp() -> u64 {
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs()
  }

  fn is_cache_valid(timestamp: u64) -> bool {
    let current_time = Self::get_current_timestamp();
    let cache_duration = 10 * 60;
    current_time - timestamp < cache_duration
  }

  pub fn load_cached_versions(&self, browser: &str) -> Option<Vec<BrowserRelease>> {
    let cache_dir = Self::get_cache_dir().ok()?;
    let cache_file = cache_dir.join(format!("{browser}_versions.json"));

    if !cache_file.exists() {
      return None;
    }

    let content = fs::read_to_string(&cache_file).ok()?;
    if let Ok(cached) = serde_json::from_str::<CachedVersionData>(&content) {
      log::info!("Using cached versions for {browser}");
      return Some(cached.releases);
    }

    if let Ok(legacy_versions) = serde_json::from_str::<Vec<String>>(&content) {
      log::info!("Using legacy cached versions for {browser}; upgrading in-memory");
      let releases: Vec<BrowserRelease> = legacy_versions
        .into_iter()
        .map(|version| BrowserRelease {
          version,
          date: "".to_string(),
        })
        .collect();
      return Some(releases);
    }

    None
  }

  pub fn is_cache_expired(&self, browser: &str) -> bool {
    let cache_dir = match Self::get_cache_dir() {
      Ok(dir) => dir,
      Err(_) => return true,
    };
    let cache_file = cache_dir.join(format!("{browser}_versions.json"));

    if !cache_file.exists() {
      return true;
    }

    let content = match fs::read_to_string(&cache_file) {
      Ok(content) => content,
      Err(_) => return true,
    };

    let cached_data: CachedVersionData = match serde_json::from_str(&content) {
      Ok(data) => data,
      Err(_) => return true,
    };

    !Self::is_cache_valid(cached_data.timestamp)
  }

  pub fn save_cached_versions(
    &self,
    browser: &str,
    releases: &[BrowserRelease],
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cache_dir = Self::get_cache_dir()?;
    let cache_file = cache_dir.join(format!("{browser}_versions.json"));

    let cached_data = CachedVersionData {
      releases: releases.to_vec(),
      timestamp: Self::get_current_timestamp(),
    };

    let content = serde_json::to_string_pretty(&cached_data)?;
    fs::write(&cache_file, content)?;
    log::info!("Cached {} versions for {}", releases.len(), browser);
    Ok(())
  }

  pub fn clear_all_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cache_dir = Self::get_cache_dir()?;

    if cache_dir.exists() {
      for entry in fs::read_dir(&cache_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
          fs::remove_file(&path)?;
          log::info!("Removed cache file: {path:?}");
        }
      }
      log::info!("All version cache cleared successfully");
    }

    Ok(())
  }
}

lazy_static::lazy_static! {
  static ref VERSION_CACHE: VersionCache = VersionCache::new();
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_version_parsing() {
    let v1 = VersionComponent::parse("1.2.3");
    assert_eq!(v1.major, 1);
    assert_eq!(v1.minor, 2);
    assert_eq!(v1.patch, 3);

    let v2 = VersionComponent::parse("138.0.7204.50");
    assert_eq!(v2.major, 138);
    assert_eq!(v2.minor, 0);
    assert_eq!(v2.patch, 7204);
    assert_eq!(v2.build, 50);

    let v3 = VersionComponent::parse("137.0b5");
    assert_eq!(v3.major, 137);
    assert_eq!(v3.minor, 0);
    assert_eq!(v3.patch, 0);
  }

  #[test]
  fn test_version_comparison() {
    assert!(VersionComponent::parse("1.2.4") > VersionComponent::parse("1.2.3"));
    assert!(VersionComponent::parse("2.0.0") > VersionComponent::parse("1.9.9"));
    assert!(VersionComponent::parse("138.0.7204.50") > VersionComponent::parse("138.0.7204.49"));
  }

  #[test]
  fn test_version_sorting() {
    let mut versions = vec![
      "138.0.7204.50".to_string(),
      "138.0.7204.49".to_string(),
      "139.0.7204.1".to_string(),
      "137.0.7204.99".to_string(),
    ];

    sort_versions(&mut versions);

    assert_eq!(versions[0], "139.0.7204.1");
    assert_eq!(versions[1], "138.0.7204.50");
    assert_eq!(versions[2], "138.0.7204.49");
    assert_eq!(versions[3], "137.0.7204.99");
  }
}
