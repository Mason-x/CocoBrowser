//! What has to happen before and after a synced profile is opened.
//!
//! Two problems this closes, both documented in `docs/SELF_HOSTING.md`:
//!
//! 1. Opening a profile used to launch straight from whatever was on local disk.
//!    Remote changes only arrived via the full reconcile at app start or the SSE
//!    subscription, so a long-running app with a dropped subscription would open
//!    stale data with no indication.
//! 2. Nothing stopped two devices opening the same profile at once, and the later
//!    close silently overwrote the earlier device's session.
//!
//! Both are best-effort against an unreachable server: a device that cannot talk
//! to the sync server still gets to open its profiles, with a warning, because
//! refusing would make the app unusable offline.

use crate::profile::BrowserProfile;
use crate::sync::device_lock::{
  device_identity, DeviceIdentity, LockAttempt, ProfileLock, LOCK_RENEW_INTERVAL_SECS,
};
use crate::sync::engine::SyncEngine;
use crate::sync::{events_emit_lock_changed, get_global_scheduler};

/// Result of the pre-launch check.
#[derive(Debug)]
pub enum LaunchGate {
  /// Sync is off for this profile, or not configured at all. Launch as before.
  NotApplicable,
  /// Lock held and remote state pulled. Clear to launch.
  Ready,
  /// Another device holds this profile. Do not launch.
  Locked(ProfileLock),
  /// Server unreachable. Launch anyway, but the user must be told the profile may
  /// be stale and is not protected against a second device opening it.
  Degraded(String),
}

/// Take the cross-device lock and pull remote changes before the browser starts.
pub async fn prepare_launch(app_handle: &tauri::AppHandle, profile: &BrowserProfile) -> LaunchGate {
  if !profile.is_sync_enabled() {
    return LaunchGate::NotApplicable;
  }

  let engine = match SyncEngine::create_from_settings(app_handle).await {
    Ok(engine) => engine,
    // Sync mode is on but the server was never configured. Not an error worth
    // blocking a launch over — the profile has nowhere to be stale relative to.
    Err(e) => {
      log::debug!(
        "Profile {} has sync enabled but no reachable configuration: {e}",
        profile.id
      );
      return LaunchGate::NotApplicable;
    }
  };

  let identity = match device_identity() {
    Ok(identity) => identity,
    Err(e) => return LaunchGate::Degraded(format!("Device identity unavailable: {e}")),
  };

  let profile_id = profile.id.to_string();

  match engine.acquire_profile_lock(&profile_id, &identity).await {
    Ok(LockAttempt::Acquired) => {}
    Ok(LockAttempt::Held(lock)) => {
      log::warn!(
        "Refusing to launch profile {} — locked by device {} ({})",
        profile_id,
        lock.device_name,
        lock.device_id
      );
      return LaunchGate::Locked(lock);
    }
    Err(e) => {
      log::warn!("Could not acquire lock for profile {profile_id}: {e}");
      return LaunchGate::Degraded(e.to_string());
    }
  }

  // Reconcile before launching. `sync_profile` skips profiles it sees running, so
  // this must happen before the process starts and before the scheduler is told
  // the profile is running.
  if let Err(e) = engine.sync_profile(app_handle, profile).await {
    log::warn!("Pre-launch sync failed for profile {profile_id}: {e}");
    // The lock is held, so the profile is at least protected; only freshness is
    // in doubt.
    return LaunchGate::Degraded(e.to_string());
  }

  events_emit_lock_changed();
  start_lock_heartbeat(app_handle.clone(), profile_id, identity);
  LaunchGate::Ready
}

/// Keep the lock alive for as long as the browser runs.
///
/// Driven off the scheduler's running set rather than its own bookkeeping, so it
/// cannot outlive the process it is protecting: the loop exits as soon as the
/// profile is no longer marked running, including after a crash-triggered cleanup.
fn start_lock_heartbeat(
  app_handle: tauri::AppHandle,
  profile_id: String,
  identity: DeviceIdentity,
) {
  tauri::async_runtime::spawn(async move {
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(LOCK_RENEW_INTERVAL_SECS)).await;

      let still_running = match get_global_scheduler() {
        Some(scheduler) => scheduler.is_profile_running(&profile_id).await,
        None => false,
      };
      if !still_running {
        break;
      }

      let engine = match SyncEngine::create_from_settings(&app_handle).await {
        Ok(engine) => engine,
        Err(e) => {
          log::debug!("Lock renewal for {profile_id} skipped: {e}");
          continue;
        }
      };

      match engine.acquire_profile_lock(&profile_id, &identity).await {
        Ok(LockAttempt::Acquired) => {
          log::debug!("Renewed lock for profile {profile_id}");
        }
        // Another device took it after ours expired — most likely this machine was
        // suspended or offline past the TTL. Keep running (killing the browser
        // would lose the user's work) but stop pretending we hold the lock.
        Ok(LockAttempt::Held(lock)) => {
          log::warn!(
            "Lock for profile {profile_id} was taken over by device {}; stopping renewal",
            lock.device_name
          );
          break;
        }
        Err(e) => log::debug!("Lock renewal for {profile_id} failed: {e}"),
      }
    }
  });
}

/// Drop the lock once the post-stop upload has finished.
///
/// Called from the scheduler rather than from the stop path on purpose. Stopping
/// the browser only *queues* the upload; releasing the lock at that moment would
/// leave a window in which another device could open the profile and pull a state
/// this device is still writing — the exact data loss the lock exists to prevent.
/// The scheduler never syncs a profile it sees running, so reaching here always
/// means the browser is down.
///
/// Only ever releases a lock this device owns, and a missing lock is not an error,
/// so it is safe to call for any profile.
pub async fn release_launch_lock(app_handle: &tauri::AppHandle, profile_id: &str) {
  let engine = match SyncEngine::create_from_settings(app_handle).await {
    Ok(engine) => engine,
    Err(_) => return,
  };
  let identity = match device_identity() {
    Ok(identity) => identity,
    Err(e) => {
      log::warn!("Cannot release profile lock without a device identity: {e}");
      return;
    }
  };

  match engine.release_profile_lock(profile_id, &identity.id).await {
    Ok(true) => {
      log::info!("Released cross-device lock for profile {profile_id}");
      events_emit_lock_changed();
    }
    Ok(false) => {}
    // The TTL is the backstop for this: the lock expires on its own.
    Err(e) => log::warn!("Failed to release lock for profile {profile_id}: {e}"),
  }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Locks currently held on any device, for display in the profile list.
///
/// Released locks are deleted rather than marked, so in normal operation this
/// lists only profiles genuinely open somewhere — usually none or one. Expired
/// entries left by a crashed device are filtered out.
#[tauri::command]
pub async fn get_profile_locks(app_handle: tauri::AppHandle) -> Result<Vec<ProfileLock>, String> {
  let engine = match SyncEngine::create_from_settings(&app_handle).await {
    Ok(engine) => engine,
    // Sync not configured: no locks exist, which is not an error to surface.
    Err(_) => return Ok(Vec::new()),
  };

  let objects = engine
    .client()
    .list_all("locks/profiles/")
    .await
    .map_err(|e| e.to_string())?;

  let mut locks = Vec::new();
  for object in objects {
    let profile_id = object
      .key
      .rsplit('/')
      .next()
      .and_then(|name| name.strip_suffix(".json"))
      .unwrap_or_default();
    if profile_id.is_empty() {
      continue;
    }
    if let Ok(Some(lock)) = engine.read_profile_lock(profile_id).await {
      locks.push(lock);
    }
  }
  Ok(locks)
}

/// This device's identity, so the UI can tell "locked here" from "locked
/// elsewhere" without a round trip.
#[tauri::command]
pub fn get_device_identity() -> Result<DeviceIdentity, String> {
  device_identity()
}

/// Delete a lock left behind by a device that died without releasing it.
///
/// Explicit user action only. If the other device is in fact still running, both
/// will write and the later close wins — which is the very thing the lock exists
/// to prevent, so the UI must say so before calling this.
#[tauri::command]
pub async fn force_release_profile_lock(
  app_handle: tauri::AppHandle,
  profile_id: String,
) -> Result<(), String> {
  let engine = SyncEngine::create_from_settings(&app_handle)
    .await
    .map_err(|_| serde_json::json!({ "code": "SYNC_NOT_CONFIGURED" }).to_string())?;

  engine
    .force_release_profile_lock(&profile_id)
    .await
    .map_err(|e| e.to_string())?;

  log::info!("Force-released cross-device lock for profile {profile_id}");
  events_emit_lock_changed();
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gate_variants_are_distinguishable_in_logs() {
    // Guards against a refactor collapsing Degraded into Ready: a degraded launch
    // must stay separable so the caller can warn instead of staying silent.
    let ready = format!("{:?}", LaunchGate::Ready);
    let degraded = format!("{:?}", LaunchGate::Degraded("offline".to_string()));
    assert_ne!(ready, degraded);
    assert!(degraded.contains("offline"));
  }

  #[test]
  fn lock_key_suffix_parsing_matches_what_list_returns() {
    let key = "locks/profiles/6f1b2c3d-0000-4000-8000-000000000001.json";
    let parsed = key
      .rsplit('/')
      .next()
      .and_then(|name| name.strip_suffix(".json"))
      .unwrap();
    assert_eq!(parsed, "6f1b2c3d-0000-4000-8000-000000000001");
  }

  #[test]
  fn non_json_keys_are_ignored() {
    let key = "locks/profiles/";
    let parsed = key
      .rsplit('/')
      .next()
      .and_then(|name| name.strip_suffix(".json"))
      .unwrap_or_default();
    assert!(parsed.is_empty());
  }
}
