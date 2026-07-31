mod client;
pub mod device_lock;
pub mod encryption;
mod engine;
pub mod launch_gate;
pub mod manifest;
pub mod scheduler;
pub mod subscription;
pub mod types;

pub use client::SyncClient;
pub use device_lock::{device_identity, DeviceIdentity, ProfileLock};
pub use encryption::{
  check_has_e2e_password, delete_e2e_password, set_e2e_password, verify_e2e_password,
};
pub use engine::{
  cancel_profile_sync, enable_extension_group_sync_if_needed, enable_group_sync_if_needed,
  enable_proxy_sync_if_needed, enable_sync_for_all_entities, enable_vpn_sync_if_needed,
  get_unsynced_entity_counts, is_group_in_use_by_synced_profile, is_group_used_by_synced_profile,
  is_proxy_in_use_by_synced_profile, is_proxy_used_by_synced_profile, is_sync_configured,
  is_vpn_in_use_by_synced_profile, is_vpn_used_by_synced_profile, pull_synced_profiles,
  request_profile_sync, rollover_encryption_for_all_entities, set_extension_group_sync_enabled,
  set_extension_sync_enabled, set_group_sync_enabled, set_profile_sync_mode,
  set_proxy_sync_enabled, set_vpn_sync_enabled, sync_all_profiles_now, sync_profile,
  trigger_sync_for_profile, SyncEngine,
};
pub use launch_gate::{
  force_release_profile_lock, get_device_identity, get_profile_locks, prepare_launch,
  release_launch_lock, LaunchGate,
};
pub use manifest::{compute_diff, generate_manifest, HashCache, ManifestDiff, SyncManifest};
pub use scheduler::{get_global_scheduler, set_global_scheduler, SyncScheduler};
pub use subscription::{SubscriptionManager, SyncWorkItem};
pub use types::{SyncError, SyncResult};

/// Tell the UI the set of cross-device locks changed so it can refetch.
///
/// A single event for all locks rather than one per profile: the list is tiny (a
/// lock only exists while a profile is open somewhere) and this keeps the frontend
/// from having to reconcile per-profile deltas.
pub fn events_emit_lock_changed() {
  let _ = crate::events::emit_empty("profile-locks-changed");
}

/// Queue a profile sync if the profile has sync enabled. No-op otherwise.
///
/// Called from profile metadata update paths so a rename / tag edit / proxy
/// reassignment shows up on other devices without waiting for the next
/// scheduled tick. Spawns the async queue call so this helper is callable
/// from both sync and async contexts.
pub fn queue_profile_sync_if_eligible(profile: &crate::profile::BrowserProfile) {
  if !profile.is_sync_enabled() {
    return;
  }
  let profile_id = profile.id.to_string();
  tauri::async_runtime::spawn(async move {
    if let Some(scheduler) = get_global_scheduler() {
      scheduler.queue_profile_sync(profile_id).await;
    }
  });
}

/// Restart the sync pipeline after the server URL or token changed.
///
/// Stops the running scheduler, then rebuilds the subscription + scheduler pair
/// against the current settings. Lived in the hosted-auth module previously
/// because it also refreshed a cloud token; sync is self-hosted only now.
#[tauri::command]
pub async fn restart_sync_service(app_handle: tauri::AppHandle) -> Result<(), String> {
  if let Some(scheduler) = get_global_scheduler() {
    scheduler.stop();
  }

  // Tells the UI its cached "is sync configured?" answer is stale. Emitted
  // before the rebuild so a device that just got its server settings can react
  // without waiting for the first sync to finish.
  let _ = crate::events::emit_empty("sync-settings-changed");

  let app_handle_sync = app_handle.clone();
  tauri::async_runtime::spawn(async move {
    let mut subscription_manager = SubscriptionManager::new();
    let work_rx = subscription_manager.take_work_receiver();

    if let Err(e) = subscription_manager.start(app_handle_sync.clone()).await {
      log::warn!("Failed to start sync subscription: {e}");
      return;
    }

    if let Some(work_rx) = work_rx {
      let scheduler = std::sync::Arc::new(SyncScheduler::new());
      set_global_scheduler(scheduler.clone());

      scheduler.sync_all_enabled_profiles(&app_handle_sync).await;

      match SyncEngine::create_from_settings(&app_handle_sync).await {
        Ok(engine) => {
          if let Err(e) = engine
            .check_for_missing_synced_profiles(&app_handle_sync)
            .await
          {
            log::warn!("Failed to check for missing profiles: {e}");
          }
          if let Err(e) = engine
            .check_for_missing_synced_entities(&app_handle_sync)
            .await
          {
            log::warn!("Failed to check for missing entities: {e}");
          }
        }
        Err(e) => {
          log::warn!("Sync not configured, skipping missing profile check: {e}");
        }
      }

      scheduler
        .clone()
        .start(app_handle_sync.clone(), work_rx)
        .await;
      log::info!("Sync scheduler restarted");
    }
  });

  Ok(())
}
