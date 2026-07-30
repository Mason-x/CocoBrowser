//! Cross-device advisory lock for opening a synced profile.
//!
//! Two devices that open the same profile both write their own copy of the
//! browser files, and whichever closes last wins the upload — the other device's
//! cookies, login state and history are lost with no error, because profile files
//! reconcile by content-hash overwrite and metadata by `updated_at`
//! last-write-wins. Neither has conflict detection. Upstream prevented this with
//! team locking, which belonged to the hosted-account layer and was removed, so
//! this is its self-hosted replacement.
//!
//! Advisory, not a mutex. Acquiring is a read-then-write with no compare-and-swap
//! (the server's object API offers none), so two devices launching inside the same
//! round trip can both believe they hold it. It closes the ordinary window — one
//! device left a profile open, or forgot to close it, and another opens it — not a
//! deliberate race.
//!
//! The lock body is deliberately NOT sealed with the E2E key. It carries no user
//! content, only which device holds it, and a device must be able to read it to
//! explain the refusal even when profiles are in `Regular` (unencrypted) mode.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::proxy_manager::now_secs;
use crate::sync::engine::SyncEngine;
use crate::sync::types::{SyncError, SyncResult};

/// How long a lock stays valid without a renewal. A device that crashes or loses
/// power blocks the profile for at most this long.
pub const LOCK_TTL_SECS: u64 = 600;

/// Renewal interval while the browser runs. Comfortably inside `LOCK_TTL_SECS` so
/// a single failed renewal (a blip, a suspended laptop) does not drop the lock.
pub const LOCK_RENEW_INTERVAL_SECS: u64 = 180;

// Two consecutive failed renewals must still land inside the TTL, or one network
// blip releases a lock on a profile that is actively open.
const _: () = assert!(LOCK_RENEW_INTERVAL_SECS * 2 < LOCK_TTL_SECS);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileLock {
  pub profile_id: String,
  pub device_id: String,
  /// Host name, shown to the user to explain which machine holds the profile.
  pub device_name: String,
  pub acquired_at: u64,
  pub expires_at: u64,
}

impl ProfileLock {
  pub fn is_expired(&self) -> bool {
    now_secs() >= self.expires_at
  }

  pub fn is_held_by(&self, device_id: &str) -> bool {
    self.device_id == device_id
  }
}

/// Outcome of trying to take the lock.
#[derive(Debug)]
pub enum LockAttempt {
  /// This device now holds it (freshly taken, or renewed because it already did).
  Acquired,
  /// Another device holds an unexpired lock.
  Held(ProfileLock),
}

fn lock_key(profile_id: &str) -> String {
  format!("locks/profiles/{profile_id}.json")
}

// ---------------------------------------------------------------------------
// Device identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
  pub id: String,
  pub name: String,
}

fn identity_path() -> PathBuf {
  crate::app_dirs::settings_dir().join("device.json")
}

fn host_name() -> String {
  sysinfo::System::host_name().unwrap_or_else(|| "unknown-device".to_string())
}

/// Stable per-installation identity, created on first use.
///
/// Kept in its own file rather than in `AppSettings` because it must never be
/// synced: if two devices shared a `device_id` the lock would not distinguish
/// them and would never refuse anything.
pub fn device_identity() -> Result<DeviceIdentity, String> {
  let path = identity_path();

  if let Ok(raw) = std::fs::read_to_string(&path) {
    if let Ok(existing) = serde_json::from_str::<DeviceIdentity>(&raw) {
      if !existing.id.is_empty() {
        // The machine may have been renamed since the file was written.
        let current = host_name();
        if existing.name != current {
          let updated = DeviceIdentity {
            id: existing.id,
            name: current,
          };
          let _ = write_identity(&path, &updated);
          return Ok(updated);
        }
        return Ok(existing);
      }
    }
  }

  let fresh = DeviceIdentity {
    id: uuid::Uuid::new_v4().to_string(),
    name: host_name(),
  };
  write_identity(&path, &fresh)?;
  Ok(fresh)
}

fn write_identity(path: &std::path::Path, identity: &DeviceIdentity) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)
      .map_err(|e| format!("Failed to create settings directory: {e}"))?;
  }
  let json = serde_json::to_string_pretty(identity)
    .map_err(|e| format!("Failed to serialize device identity: {e}"))?;
  std::fs::write(path, json).map_err(|e| format!("Failed to write device identity: {e}"))?;
  crate::app_dirs::restrict_to_owner(path);
  Ok(())
}

// ---------------------------------------------------------------------------
// Remote operations
// ---------------------------------------------------------------------------

impl SyncEngine {
  /// Read the current lock, treating absent and expired alike as "not held".
  pub async fn read_profile_lock(&self, profile_id: &str) -> SyncResult<Option<ProfileLock>> {
    let key = lock_key(profile_id);

    let stat = self.client().stat(&key).await?;
    if !stat.exists {
      return Ok(None);
    }

    let presign = self.client().presign_download(&key).await?;
    let raw = self.client().download_bytes(&presign.url).await?;
    let lock: ProfileLock = serde_json::from_slice(&raw)
      .map_err(|e| SyncError::InvalidData(format!("Malformed profile lock at {key}: {e}")))?;

    if lock.is_expired() {
      return Ok(None);
    }
    Ok(Some(lock))
  }

  /// Take the lock, or report who holds it.
  ///
  /// Renewing an existing lock this device already owns is the same call, so the
  /// heartbeat while the browser runs reuses it.
  pub async fn acquire_profile_lock(
    &self,
    profile_id: &str,
    identity: &DeviceIdentity,
  ) -> SyncResult<LockAttempt> {
    if let Some(existing) = self.read_profile_lock(profile_id).await? {
      if !existing.is_held_by(&identity.id) {
        return Ok(LockAttempt::Held(existing));
      }
    }

    let now = now_secs();
    let lock = ProfileLock {
      profile_id: profile_id.to_string(),
      device_id: identity.id.clone(),
      device_name: identity.name.clone(),
      acquired_at: now,
      expires_at: now + LOCK_TTL_SECS,
    };

    let body = serde_json::to_vec(&lock)
      .map_err(|e| SyncError::InvalidData(format!("Failed to serialize profile lock: {e}")))?;
    let key = lock_key(profile_id);
    let presign = self
      .client()
      .presign_upload(&key, Some("application/json"))
      .await?;
    self
      .client()
      .upload_bytes(&presign.url, &body, Some("application/json"))
      .await?;

    Ok(LockAttempt::Acquired)
  }

  /// Drop the lock, but only if this device owns it — a device must never release
  /// another device's lock just because it stopped its own browser.
  pub async fn release_profile_lock(&self, profile_id: &str, device_id: &str) -> SyncResult<bool> {
    match self.read_profile_lock(profile_id).await? {
      Some(lock) if lock.is_held_by(device_id) => {
        self.client().delete(&lock_key(profile_id), None).await?;
        Ok(true)
      }
      // Already gone, expired, or someone else's: nothing to do.
      _ => Ok(false),
    }
  }

  /// Delete the lock regardless of holder. Only for the explicit user action
  /// taken when a device died without releasing and waiting out the TTL is not
  /// acceptable.
  pub async fn force_release_profile_lock(&self, profile_id: &str) -> SyncResult<()> {
    self.client().delete(&lock_key(profile_id), None).await?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn lock_with(expires_in: i64, device: &str) -> ProfileLock {
    let now = now_secs();
    ProfileLock {
      profile_id: "p1".to_string(),
      device_id: device.to_string(),
      device_name: "host".to_string(),
      acquired_at: now,
      expires_at: (now as i64 + expires_in).max(0) as u64,
    }
  }

  #[test]
  fn lock_key_is_namespaced_per_profile() {
    assert_eq!(lock_key("abc"), "locks/profiles/abc.json");
    assert_ne!(lock_key("abc"), lock_key("xyz"));
  }

  #[test]
  fn future_expiry_is_not_expired() {
    assert!(!lock_with(300, "d1").is_expired());
  }

  #[test]
  fn past_expiry_is_expired() {
    assert!(lock_with(-1, "d1").is_expired());
  }

  #[test]
  fn expiry_exactly_now_is_expired() {
    let mut lock = lock_with(0, "d1");
    lock.expires_at = now_secs();
    assert!(lock.is_expired());
  }

  #[test]
  fn holder_check_is_exact() {
    let lock = lock_with(300, "device-a");
    assert!(lock.is_held_by("device-a"));
    assert!(!lock.is_held_by("device-b"));
    assert!(!lock.is_held_by("device-a "));
    assert!(!lock.is_held_by("DEVICE-A"));
  }

  #[test]
  fn identity_round_trips_and_keeps_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(dir.path().to_path_buf());

    let first = device_identity().unwrap();
    assert!(!first.id.is_empty());
    assert!(!first.name.is_empty());

    let second = device_identity().unwrap();
    assert_eq!(
      first.id, second.id,
      "device id must be stable across calls or the lock cannot recognise itself"
    );
  }

  #[test]
  fn identity_is_regenerated_when_the_file_is_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(dir.path().to_path_buf());

    let first = device_identity().unwrap();
    std::fs::write(identity_path(), b"{ not json").unwrap();

    let second = device_identity().unwrap();
    assert!(!second.id.is_empty());
    assert_ne!(first.id, second.id);
  }

  #[test]
  fn identity_with_an_empty_id_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(dir.path().to_path_buf());

    std::fs::create_dir_all(identity_path().parent().unwrap()).unwrap();
    std::fs::write(identity_path(), br#"{"id":"","name":"x"}"#).unwrap();

    let identity = device_identity().unwrap();
    assert!(!identity.id.is_empty());
  }
}
