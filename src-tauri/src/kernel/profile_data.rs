//! Version stamp Chromium keeps on a user-data-dir.
//!
//! Chromium writes the build that last opened a profile into a `Last Version`
//! file at the root of the user data dir, and refuses to start when it finds a
//! stamp newer than the running binary. Moving a profile onto an older kernel
//! therefore needs a warning before the switch and `--allow-profile-downgrade`
//! on every launch that follows, until a newer build re-stamps the dir.

use std::path::Path;

const LAST_VERSION_FILE: &str = "Last Version";

/// Version string stamped on the profile dir, when there is a readable one.
pub fn last_version(user_data_dir: &Path) -> Option<String> {
  let text = std::fs::read_to_string(user_data_dir.join(LAST_VERSION_FILE)).ok()?;
  let trimmed = text.trim();
  if trimmed.is_empty() {
    None
  } else {
    Some(trimmed.to_string())
  }
}

/// Leading major of a Chromium version string (`"150.0.7401.9"` → `150`).
pub fn major_of(version: &str) -> Option<u32> {
  version.split('.').next()?.parse().ok()
}

/// True when `target_version` is older than the build that last opened this
/// profile dir — the case Chromium blocks without `--allow-profile-downgrade`.
///
/// An unstamped or unparsable dir is never a downgrade: a profile that no
/// browser has opened yet has nothing to be older than.
pub fn is_downgrade(user_data_dir: &Path, target_version: &str) -> bool {
  match (
    last_version(user_data_dir).as_deref().and_then(major_of),
    major_of(target_version),
  ) {
    (Some(stamped), Some(target)) => target < stamped,
    _ => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn stamp(dir: &TempDir, value: &str) {
    std::fs::write(dir.path().join(LAST_VERSION_FILE), value).unwrap();
  }

  #[test]
  fn reads_and_trims_the_stamp() {
    let dir = TempDir::new().unwrap();
    stamp(&dir, "150.0.7401.9\n");
    assert_eq!(last_version(dir.path()).as_deref(), Some("150.0.7401.9"));
  }

  #[test]
  fn unstamped_dir_has_no_version_and_is_never_a_downgrade() {
    let dir = TempDir::new().unwrap();
    assert_eq!(last_version(dir.path()), None);
    assert!(!is_downgrade(dir.path(), "146"));
  }

  #[test]
  fn detects_only_backwards_moves() {
    let dir = TempDir::new().unwrap();
    stamp(&dir, "150.0.7401.9");
    assert!(is_downgrade(dir.path(), "146"));
    assert!(is_downgrade(dir.path(), "148.0.7778.215"));
    assert!(!is_downgrade(dir.path(), "150"));
    assert!(!is_downgrade(dir.path(), "151.0.1.0"));
  }

  #[test]
  fn unparsable_stamp_is_ignored() {
    let dir = TempDir::new().unwrap();
    stamp(&dir, "not-a-version");
    assert!(!is_downgrade(dir.path(), "146"));
  }
}
