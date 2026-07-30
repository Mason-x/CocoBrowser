use crate::browser::ProxySettings;
use crate::downloaded_browsers_registry::DownloadedBrowsersRegistry;
use crate::events;
use crate::kernel::{AutomationMode, KernelLaunchRequest, KernelRegistry, LocalProxyEndpoint};
use crate::profile::{BrowserProfile, ProfileManager};
use crate::proxy_manager::PROXY_MANAGER;
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct BrowserRunner {
  pub profile_manager: &'static ProfileManager,
  pub downloaded_browsers_registry: &'static DownloadedBrowsersRegistry,
  kernel_registry: &'static KernelRegistry,
}

impl BrowserRunner {
  fn new() -> Self {
    Self {
      profile_manager: ProfileManager::instance(),
      downloaded_browsers_registry: DownloadedBrowsersRegistry::instance(),
      kernel_registry: KernelRegistry::instance(),
    }
  }

  pub fn instance() -> &'static BrowserRunner {
    &BROWSER_RUNNER
  }

  /// Resolve the DNS blocklist level to a cached file path.
  /// If a level is set but the cache is missing, fetches on demand (blocks until done).
  async fn resolve_blocklist_file(
    profile: &crate::profile::BrowserProfile,
  ) -> Result<Option<String>, String> {
    let Some(ref level_str) = profile.dns_blocklist else {
      return Ok(None);
    };
    let Some(level) = crate::dns_blocklist::BlocklistLevel::parse_level(level_str) else {
      return Ok(None);
    };
    if level == crate::dns_blocklist::BlocklistLevel::None {
      return Ok(None);
    }
    let path = crate::dns_blocklist::BlocklistManager::ensure_cached(level)
      .await
      .map_err(|e| format!("Failed to fetch DNS blocklist: {e}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
  }

  fn fire_launch_hook(profile: &BrowserProfile) {
    let Some(raw_url) = profile.launch_hook.as_deref() else {
      return;
    };
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
      return;
    }

    let parsed = match url::Url::parse(trimmed) {
      Ok(u) => u,
      Err(e) => {
        log::warn!(
          "Skipping launch hook for profile {} (ID: {}): invalid URL: {e}",
          profile.name,
          profile.id
        );
        return;
      }
    };

    if !matches!(parsed.scheme(), "http" | "https") {
      log::warn!(
        "Skipping launch hook for profile {} (ID: {}): URL must be http or https",
        profile.name,
        profile.id
      );
      return;
    }

    let url = parsed.to_string();
    let profile_name = profile.name.clone();
    let profile_id = profile.id.to_string();

    log::info!("Firing launch hook GET {url} for profile {profile_name} (ID: {profile_id})");

    tokio::spawn(async move {
      let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
      {
        Ok(c) => c,
        Err(e) => {
          log::warn!("Launch hook client build failed for {url}: {e}");
          return;
        }
      };

      match client.get(&url).send().await {
        Ok(resp) => {
          log::info!(
            "Launch hook {url} for profile {profile_name} returned status {}",
            resp.status()
          );
        }
        Err(e) => {
          log::warn!("Launch hook {url} for profile {profile_name} failed: {e}");
        }
      }
    });
  }

  pub async fn launch_browser(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
    url: Option<String>,
    local_proxy_settings: Option<&ProxySettings>,
  ) -> Result<BrowserProfile, Box<dyn std::error::Error + Send + Sync>> {
    self
      .launch_browser_internal(app_handle, profile, url, local_proxy_settings, None, false)
      .await
  }

  async fn launch_browser_internal(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
    url: Option<String>,
    _local_proxy_settings: Option<&ProxySettings>,
    remote_debugging_port: Option<u16>,
    headless: bool,
  ) -> Result<BrowserProfile, Box<dyn std::error::Error + Send + Sync>> {
    // fingerprint-chromium: Persona + Job Object + geo gate via KernelDriver.
    if profile.browser == "fingerprint-chromium" {
      use crate::kernel::geo_consistency::{
        check_geo_consistency, match_persona_to_exit, reject_cloud_proxy_id, GeoGateResult,
      };
      use crate::kernel::persona::ensure_persona;
      use crate::kernel::session::SessionManager;

      if SessionManager::instance().is_running(profile.id) {
        return Err(format!("profile {} is already launching or running", profile.name).into());
      }

      // The launch hook previously fired only from the legacy engine's proxy
      // resolution, so profiles on this kernel never triggered it even though
      // the setting is exposed in the UI. Fire it here, before any launch work.
      Self::fire_launch_hook(profile);

      // Local-first: refuse cloud-managed proxies (no Coco cloud refresh).
      reject_cloud_proxy_id(profile.proxy_id.as_deref())
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

      let mut updated_profile = profile.clone();
      let mut persona = ensure_persona(updated_profile.persona.as_ref(), &updated_profile.version)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
      if updated_profile.persona.as_ref() != Some(&persona) {
        updated_profile.persona = Some(persona.clone());
        self.save_process_info(&updated_profile)?;
      }

      // Resolve stored proxy without cloud credential refresh.
      let mut upstream_proxy: Option<ProxySettings> = None;
      if let Some(ref proxy_id) = profile.proxy_id {
        upstream_proxy = PROXY_MANAGER.get_proxy_settings_by_id(proxy_id);
        if upstream_proxy.is_none() {
          return Err(format!("Stored proxy not found: {proxy_id}").into());
        }
      }

      if upstream_proxy.is_none() {
        if let Some(ref vpn_id) = profile.vpn_id {
          match crate::vpn_worker_runner::start_vpn_worker(vpn_id).await {
            Ok(vpn_worker) => {
              if let Some(port) = vpn_worker.local_port {
                upstream_proxy = Some(ProxySettings {
                  proxy_type: "socks5".to_string(),
                  host: "127.0.0.1".to_string(),
                  port,
                  username: None,
                  password: None,
                });
              }
            }
            Err(e) => return Err(format!("Failed to start VPN worker: {e}").into()),
          }
        }
      }

      let profile_id_str = profile.id.to_string();
      let blocklist_file = Self::resolve_blocklist_file(profile).await?;
      let local_proxy = PROXY_MANAGER
        .start_proxy(
          app_handle.clone(),
          upstream_proxy.as_ref(),
          0,
          Some(&profile_id_str),
          profile.proxy_bypass_rules.clone(),
          blocklist_file,
          "socks5",
        )
        .await
        .map_err(|e| format!("Failed to start local proxy: {e}"))?;

      let local_proxy_endpoint = LocalProxyEndpoint {
        host: local_proxy.host.clone(),
        port: local_proxy.port,
        protocol: "socks5".to_string(),
      };

      // GeoIP data is useful for timezone matching, but its first download
      // must not turn a working proxy into a browser-launch failure. Start the
      // download in the background; check_geo_consistency will still verify
      // reachability now and defer only the timezone portion when the file is
      // not ready yet.
      if !crate::geoip_downloader::GeoIPDownloader::is_geoip_database_available() {
        let geoip_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
          if let Err(error) = crate::geoip_downloader::GeoIPDownloader::instance()
            .download_geoip_database(&geoip_handle)
            .await
          {
            log::warn!("Background GeoIP download failed: {error}");
          }
        });
      }

      // Geo gate: exit IP via the same local sidecar; explicit mismatches and
      // real proxy failures still fail closed.
      // Upstream credentials only live in the sidecar process, never on the CLI.
      let gate = check_geo_consistency(
        &persona,
        Some(&local_proxy_endpoint),
        upstream_proxy.as_ref(),
        profile.vpn_id.as_deref(),
        profile.proxy_id.as_deref(),
      )
      .await;

      match gate {
        GeoGateResult::CloudProxyRejected => {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(
            serde_json::json!({ "code": "CLOUD_PROXY_NOT_SUPPORTED" })
              .to_string()
              .into(),
          );
        }
        GeoGateResult::ProxyFailed { reason } => {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(
            serde_json::json!({
              "code": "PROXY_EXIT_CHECK_FAILED",
              "params": { "reason": reason }
            })
            .to_string()
            .into(),
          );
        }
        GeoGateResult::Blocked {
          observation,
          reason,
        } => {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(
            serde_json::json!({
              "code": "GEO_TIMEZONE_MISMATCH",
              "params": {
                "reason": reason,
                "exitTimezone": observation.timezone,
                "personaTimezone": persona.timezone,
                "exitIp": observation.exit_ip
              }
            })
            .to_string()
            .into(),
          );
        }
        GeoGateResult::Pass { observation } => {
          if let Some(obs) = observation {
            // Stamp signature when timezone already matches so next launch is cheap.
            if persona.proxy_geo_signature.as_deref() != Some(obs.signature.as_str()) {
              if persona.timezone == obs.timezone {
                persona.proxy_geo_signature = Some(obs.signature.clone());
                updated_profile.persona = Some(persona.clone());
                let _ = self.save_process_info(&updated_profile);
              } else {
                // Should not happen if evaluate_gate is consistent; match explicitly.
                match_persona_to_exit(&mut persona, &obs);
                updated_profile.persona = Some(persona.clone());
                let _ = self.save_process_info(&updated_profile);
              }
            }
          }
        }
      }

      if profile.password_protected {
        if let Err(error) = crate::profile::password::prepare_for_launch(profile) {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(error.into());
        }
      } else if profile.ephemeral {
        if let Err(error) = crate::ephemeral_dirs::create_ephemeral_dir(&profile.id.to_string()) {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(error.into());
        }
      }

      let profiles_dir = self.profile_manager.get_profiles_dir();
      let profile_data_path =
        crate::ephemeral_dirs::get_effective_profile_path(&updated_profile, &profiles_dir);

      let mut extension_paths = Vec::new();
      // Overrides the new tab page, so the browser opens on the workbench with an
      // empty address bar. Only when the caller asked for no particular URL —
      // an automation client that wants a page should get that page.
      if url.is_none() {
        if let Some(dir) = crate::workbench::extension_if_enabled(&profile_id_str) {
          extension_paths.push(dir);
        }
      }
      if updated_profile.extension_group_id.is_some() {
        let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
        if let Ok(paths) = mgr.install_extensions_for_profile(&updated_profile, &profile_data_path)
        {
          extension_paths = paths;
        }
      }

      let automation = if headless {
        AutomationMode::Headless
      } else if remote_debugging_port.is_some() {
        AutomationMode::HeadedAutomation
      } else {
        AutomationMode::Manual
      };

      let launch_request = KernelLaunchRequest {
        profile: updated_profile.clone(),
        profile_path: profile_data_path,
        url: url.clone(),
        local_proxy: Some(local_proxy_endpoint),
        automation,
        remote_debugging_port,
        headless,
        extension_paths,
        persona: Some(persona),
        proxy_url: Some(format!(
          "socks5://{}:{}",
          local_proxy.host, local_proxy.port
        )),
        ephemeral: profile.ephemeral,
      };

      let kernel = match self.kernel_registry.require("fingerprint-chromium") {
        Ok(kernel) => kernel,
        Err(error) => {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(error.into());
        }
      };

      let result = match kernel.launch(&app_handle, launch_request).await {
        Ok(r) => r,
        Err(e) => {
          let _ = PROXY_MANAGER
            .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
            .await;
          return Err(format!("Failed to launch fingerprint-chromium: {e}").into());
        }
      };

      let process_id = result.pid.unwrap_or(0);
      updated_profile.process_id = Some(process_id);
      updated_profile.last_launch = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());

      if let Err(e) =
        PROXY_MANAGER.update_proxy_pid_for_profile(&updated_profile.id.to_string(), process_id)
      {
        log::warn!("Failed to update proxy PID mapping: {e}");
      }
      PROXY_MANAGER.set_browser_pid_for_profile(&updated_profile.id.to_string(), process_id);
      if let Err(error) = self.save_process_info(&updated_profile) {
        let _ = kernel.stop(&result).await;
        let _ = PROXY_MANAGER
          .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
          .await;
        return Err(error);
      }

      if let Err(e) = events::emit_empty("profiles-changed") {
        log::warn!("Failed to emit profiles-changed: {e}");
      }
      if let Err(e) = events::emit("profile-updated", &updated_profile) {
        log::warn!("Failed to emit profile-updated: {e}");
      }

      #[derive(Serialize)]
      struct RunningChangedPayload {
        id: String,
        is_running: bool,
      }
      let _ = events::emit(
        "profile-running-changed",
        &RunningChangedPayload {
          id: updated_profile.id.to_string(),
          is_running: true,
        },
      );

      return Ok(updated_profile);
    }

    Err(format!("Unsupported browser type: {}", profile.browser).into())
  }

  pub async fn open_url_in_existing_browser(
    &self,
    _app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
    _url: &str,
    _internal_proxy_settings: Option<&ProxySettings>,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err(format!("Unsupported browser type: {}", profile.browser).into())
  }

  pub async fn launch_browser_with_debugging(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
    url: Option<String>,
    remote_debugging_port: Option<u16>,
    headless: bool,
  ) -> Result<BrowserProfile, Box<dyn std::error::Error + Send + Sync>> {
    // Wayfern starts (and PID-reconciles) its own local proxy
    // inside `launch_browser_internal`, so we hand it None here rather than
    // staging a second, orphaned proxy worker.
    self
      .launch_browser_internal(
        app_handle,
        profile,
        url,
        None,
        remote_debugging_port,
        headless,
      )
      .await
  }

  pub async fn launch_or_open_url(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
    url: Option<String>,
    internal_proxy_settings: Option<&ProxySettings>,
  ) -> Result<BrowserProfile, Box<dyn std::error::Error + Send + Sync>> {
    log::info!(
      "launch_or_open_url called for profile: {} (ID: {})",
      profile.name,
      profile.id
    );

    // Get the most up-to-date profile data
    let profiles = self
      .profile_manager
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles in launch_or_open_url: {e}"))?;
    let updated_profile = profiles
      .into_iter()
      .find(|p| p.id == profile.id)
      .unwrap_or_else(|| profile.clone());

    log::info!(
      "Checking browser status for profile: {} (ID: {})",
      updated_profile.name,
      updated_profile.id
    );

    // Check if browser is already running
    let is_running = self
      .check_browser_status(app_handle.clone(), &updated_profile)
      .await
      .map_err(|e| format!("Failed to check browser status: {e}"))?;

    // Get the updated profile again after status check (PID might have been updated)
    let profiles = self
      .profile_manager
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles after status check: {e}"))?;
    let final_profile = profiles
      .into_iter()
      .find(|p| p.id == profile.id)
      .unwrap_or_else(|| updated_profile.clone());

    log::info!(
      "Browser status check - Profile: {} (ID: {}), Running: {}, URL: {:?}, PID: {:?}",
      final_profile.name,
      final_profile.id,
      is_running,
      url,
      final_profile.process_id
    );

    if is_running && url.is_some() {
      // Browser is running and we have a URL to open
      if let Some(url_ref) = url.as_ref() {
        log::info!("Opening URL in existing browser: {url_ref}");

        match self
          .open_url_in_existing_browser(
            app_handle.clone(),
            &final_profile,
            url_ref,
            internal_proxy_settings,
          )
          .await
        {
          Ok(()) => {
            log::info!("Successfully opened URL in existing browser");
            Ok(final_profile)
          }
          Err(e) => {
            log::info!("Failed to open URL in existing browser: {e}");

            // Fall back to launching a new instance
            log::info!(
              "Falling back to new instance for browser: {}",
              final_profile.browser
            );
            // Fallback to launching a new instance for other browsers
            self
              .launch_browser_internal(
                app_handle.clone(),
                &final_profile,
                url,
                internal_proxy_settings,
                None,
                false,
              )
              .await
          }
        }
      } else {
        // This case shouldn't happen since we checked is_some() above, but handle it gracefully
        log::info!("URL was unexpectedly None, launching new browser instance");
        self
          .launch_browser(
            app_handle.clone(),
            &final_profile,
            url,
            internal_proxy_settings,
          )
          .await
      }
    } else {
      // Browser is not running or no URL provided, launch new instance
      if !is_running {
        log::info!("Launching new browser instance - browser not running");
      } else {
        log::info!("Launching new browser instance - no URL provided");
      }
      self
        .launch_browser_internal(
          app_handle.clone(),
          &final_profile,
          url,
          internal_proxy_settings,
          None,
          false,
        )
        .await
    }
  }

  fn save_process_info(
    &self,
    profile: &BrowserProfile,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Use the regular save_profile method which handles the UUID structure
    self.profile_manager.save_profile(profile).map_err(|e| {
      let error_string = e.to_string();
      Box::new(std::io::Error::other(error_string)) as Box<dyn std::error::Error + Send + Sync>
    })
  }

  pub async fn check_browser_status(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
  ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let running = self
      .profile_manager
      .check_browser_status(app_handle, profile)
      .await?;

    // Closing the browser window never goes through `kill_browser_process`, so
    // nothing moved the session out of Running and every later launch was refused
    // with "already launching or running". This poll is the only place that sees
    // the process disappear on its own, so it is where the session is released.
    if !running {
      let _ = crate::kernel::session::SessionManager::instance()
        .set_state(profile.id, crate::kernel::session::SessionState::Stopped);
    }

    Ok(running)
  }

  pub async fn kill_browser_process(
    &self,
    app_handle: tauri::AppHandle,
    profile: &BrowserProfile,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // fingerprint-chromium: stop via KernelDriver Job Object tree only.
    if profile.browser == "fingerprint-chromium" {
      let profile_id_str = profile.id.to_string();
      if let Err(e) = PROXY_MANAGER
        .stop_proxy_by_profile_id(app_handle.clone(), &profile_id_str)
        .await
      {
        log::warn!("Failed to stop proxy for profile {profile_id_str}: {e}");
      }

      let kernel = self
        .kernel_registry
        .require("fingerprint-chromium")
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
      let process = crate::kernel::BrowserProcess {
        profile_id: profile_id_str.clone(),
        kernel_id: "fingerprint-chromium".into(),
        pid: profile.process_id,
        created_at: None,
        cdp_port: None,
        user_data_dir: crate::ephemeral_dirs::get_effective_profile_path(
          profile,
          &self.profile_manager.get_profiles_dir(),
        ),
        instance_id: profile.process_id.map(|pid| format!("fchromium-{pid}")),
        used_fingerprint: None,
      };
      kernel
        .stop(&process)
        .await
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

      let mut updated = profile.clone();
      updated.process_id = None;
      self.save_process_info(&updated)?;
      let _ = events::emit("profile-updated", &updated);
      #[derive(Serialize)]
      struct RunningChangedPayload {
        id: String,
        is_running: bool,
      }
      let _ = events::emit(
        "profile-running-changed",
        &RunningChangedPayload {
          id: profile.id.to_string(),
          is_running: false,
        },
      );
      return Ok(());
    }

    Err(
      format!(
        "Unsupported browser '{}' for profile '{}' — only Wayfern is supported",
        profile.browser, profile.name
      )
      .into(),
    )
  }

  pub async fn open_url_with_profile(
    &self,
    app_handle: tauri::AppHandle,
    profile_id: String,
    url: String,
  ) -> Result<(), String> {
    // Get the profile by name
    let profiles = self
      .profile_manager
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles: {e}"))?;
    let profile = profiles
      .into_iter()
      .find(|p| p.id.to_string() == profile_id)
      .ok_or_else(|| format!("Profile '{profile_id}' not found"))?;

    if profile.is_cross_os() {
      return Err(format!(
        "Cannot open URL with profile '{}': this profile was created on {} and cannot be used on a different operating system",
        profile.name,
        profile.host_os.as_deref().unwrap_or("another OS"),
      ));
    }

    log::info!("Opening URL '{url}' with profile '{profile_id}'");

    // Use launch_or_open_url which handles both launching new instances and opening in existing ones
    self
      .launch_or_open_url(app_handle, &profile, Some(url.clone()), None)
      .await
      .map_err(|e| {
        log::info!("Failed to open URL with profile '{profile_id}': {e}");
        format!("Failed to open URL with profile: {e}")
      })?;

    log::info!("Successfully opened URL '{url}' with profile '{profile_id}'");
    Ok(())
  }
}

#[tauri::command]
pub async fn launch_browser_profile(
  app_handle: tauri::AppHandle,
  profile: BrowserProfile,
  url: Option<String>,
) -> Result<BrowserProfile, String> {
  launch_browser_profile_impl(app_handle, profile, url, None, false, false).await
}

pub async fn launch_browser_profile_impl(
  app_handle: tauri::AppHandle,
  profile: BrowserProfile,
  url: Option<String>,
  remote_debugging_port: Option<u16>,
  headless: bool,
  force_new: bool,
) -> Result<BrowserProfile, String> {
  log::info!(
    "Launch request received for profile: {} (ID: {})",
    profile.name,
    profile.id
  );

  if profile.is_cross_os() {
    return Err(format!(
      "Cannot launch profile '{}': this profile was created on {} and cannot be launched on a different operating system",
      profile.name,
      profile.host_os.as_deref().unwrap_or("another OS"),
    ));
  }

  // Take the cross-device lock and pull remote changes BEFORE the process starts.
  // Order matters twice over: `sync_profile` skips profiles it sees running, and
  // the scheduler must not be told the profile is running until the pull is done.
  match crate::sync::prepare_launch(&app_handle, &profile).await {
    crate::sync::LaunchGate::NotApplicable | crate::sync::LaunchGate::Ready => {}
    crate::sync::LaunchGate::Locked(lock) => {
      return Err(
        serde_json::json!({
          "code": "PROFILE_LOCKED_BY_DEVICE",
          "params": { "device": lock.device_name }
        })
        .to_string(),
      );
    }
    // Offline, or the server rejected us. Launching is still the right call —
    // refusing would make the app unusable without network — but the profile may
    // be stale and is not protected from a second device opening it.
    crate::sync::LaunchGate::Degraded(reason) => {
      log::warn!(
        "Launching profile {} without a verified sync state: {reason}",
        profile.id
      );
      let _ = crate::events::emit(
        "profile-launch-sync-degraded",
        serde_json::json!({
          "profile_id": profile.id.to_string(),
          "profile_name": profile.name,
        }),
      );
    }
  }

  // Notify sync scheduler that profile is now running and queue sync for when it stops
  if let Some(scheduler) = crate::sync::get_global_scheduler() {
    let pid = profile.id.to_string();
    scheduler.mark_profile_running(&pid).await;
    if profile.is_sync_enabled() {
      scheduler.queue_profile_sync(pid).await;
    }
  }

  let browser_runner = BrowserRunner::instance();

  // Resolve the most up-to-date profile from disk by ID to avoid using stale proxy_id/browser state
  let profile_for_launch = match browser_runner
    .profile_manager
    .list_profiles()
    .map_err(|e| format!("Failed to list profiles: {e}"))
  {
    Ok(profiles) => profiles
      .into_iter()
      .find(|p| p.id == profile.id)
      .unwrap_or_else(|| profile.clone()),
    Err(e) => {
      return Err(e);
    }
  };

  log::info!(
    "Resolved profile for launch: {} (ID: {})",
    profile_for_launch.name,
    profile_for_launch.id
  );

  log::info!(
    "Starting browser launch for profile: {} (ID: {})",
    profile_for_launch.name,
    profile_for_launch.id
  );

  // Launch browser or open URL in existing instance. Wayfern starts its
  // own local proxy inside `launch_browser_internal`; other browser types
  // are rejected there, so no proxy needs to be staged here.
  //
  // `force_new` callers (API/MCP) always start a fresh instance with the
  // requested debug port and headless mode, bypassing the "open URL in the
  // existing window" path which would otherwise ignore both.
  let launch_result = if force_new {
    browser_runner
      .launch_browser_with_debugging(
        app_handle.clone(),
        &profile_for_launch,
        url,
        remote_debugging_port,
        headless,
      )
      .await
  } else {
    browser_runner
      .launch_or_open_url(app_handle.clone(), &profile_for_launch, url, None)
      .await
  };
  let updated_profile = launch_result.map_err(|e| {
    log::info!("Browser launch failed for profile: {}, error: {}", profile_for_launch.name, e);

    // Emit a failure event to clear loading states in the frontend
    #[derive(serde::Serialize)]
    struct RunningChangedPayload {
      id: String,
      is_running: bool,
    }
    let payload = RunningChangedPayload {
      id: profile_for_launch.id.to_string(),
      is_running: false,
    };

    if let Err(e) = events::emit("profile-running-changed", &payload) {
      log::warn!("Warning: Failed to emit profile running changed event: {e}");
    }

    // Check if this is an architecture compatibility issue
    if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
      if io_error.kind() == std::io::ErrorKind::Other && io_error.to_string().contains("Exec format error") {
        return format!("Failed to launch browser: Executable format error. This browser version is not compatible with your system architecture ({}). Please try a different browser or version that supports your platform.", std::env::consts::ARCH);
      }
    }
    format!("Failed to launch browser or open URL: {e}")
  })?;

  log::info!(
    "Browser launch completed for profile: {} (ID: {})",
    updated_profile.name,
    updated_profile.id
  );

  // Now update the proxy with the correct PID if we have one
  if let Some(actual_pid) = updated_profile.process_id {
    // Update the proxy manager with the correct PID (we always started with temp pid 1)
    let _ = PROXY_MANAGER.update_proxy_pid(1u32, actual_pid);
  }

  Ok(updated_profile)
}

#[tauri::command]
pub fn check_browser_exists(browser_str: String, version: String) -> bool {
  // This is an alias for is_browser_downloaded to provide clearer semantics for auto-updates
  let runner = BrowserRunner::instance();
  runner
    .downloaded_browsers_registry
    .is_browser_downloaded(&browser_str, &version)
}

#[tauri::command]
pub async fn kill_browser_profile(
  app_handle: tauri::AppHandle,
  profile: BrowserProfile,
) -> Result<(), String> {
  log::info!(
    "Kill request received for profile: {} (ID: {})",
    profile.name,
    profile.id
  );

  let browser_runner = BrowserRunner::instance();

  match browser_runner
    .kill_browser_process(app_handle.clone(), &profile)
    .await
  {
    Ok(()) => {
      log::info!(
        "Successfully killed browser profile: {} (ID: {})",
        profile.name,
        profile.id
      );

      // Notify sync scheduler that profile stopped, which makes the queued upload
      // eligible. The cross-device lock is deliberately NOT released here — it is
      // released once that upload finishes, in the scheduler, so another device
      // cannot start pulling while this one is still writing.
      if let Some(scheduler) = crate::sync::get_global_scheduler() {
        scheduler
          .mark_profile_stopped(&profile.id.to_string())
          .await;
      }

      // Auto-update non-running profiles and cleanup unused binaries
      let browser_for_update = profile.browser.clone();
      let app_handle_for_update = app_handle.clone();
      tauri::async_runtime::spawn(async move {
        let registry = crate::downloaded_browsers_registry::DownloadedBrowsersRegistry::instance();
        let mut versions = registry.get_downloaded_versions(&browser_for_update);
        if !versions.is_empty() {
          versions.sort_by(|a, b| crate::version_cache::compare_versions(b, a));
          let latest_version = &versions[0];

          let auto_updater = crate::auto_updater::AutoUpdater::instance();
          match auto_updater
            .auto_update_profile_versions(
              &app_handle_for_update,
              &browser_for_update,
              latest_version,
            )
            .await
          {
            Ok(updated) => {
              if !updated.is_empty() {
                log::info!(
                  "Auto-updated {} profiles after stop: {:?}",
                  updated.len(),
                  updated
                );
              }
            }
            Err(e) => {
              log::error!("Failed to auto-update profile versions after stop: {e}");
            }
          }
        }

        match registry.cleanup_unused_binaries() {
          Ok(cleaned) => {
            if !cleaned.is_empty() {
              log::info!("Cleaned up unused binaries after stop: {:?}", cleaned);
            }
          }
          Err(e) => {
            log::error!("Failed to cleanup unused binaries after stop: {e}");
          }
        }
      });

      Ok(())
    }
    Err(e) => {
      log::info!("Failed to kill browser profile {}: {}", profile.name, e);

      // Emit a failure event to clear loading states in the frontend
      #[derive(serde::Serialize)]
      struct RunningChangedPayload {
        id: String,
        is_running: bool,
      }
      // On kill failure, we assume the process is still running
      let payload = RunningChangedPayload {
        id: profile.id.to_string(),
        is_running: true,
      };

      if let Err(e) = events::emit("profile-running-changed", &payload) {
        log::warn!("Warning: Failed to emit profile running changed event: {e}");
      }

      Err(format!("Failed to kill browser: {e}"))
    }
  }
}

#[tauri::command]
pub async fn open_url_with_profile(
  app_handle: tauri::AppHandle,
  profile_id: String,
  url: String,
) -> Result<(), String> {
  let browser_runner = BrowserRunner::instance();
  browser_runner
    .open_url_with_profile(app_handle, profile_id, url)
    .await
}

// Global singleton instance
lazy_static::lazy_static! {
  static ref BROWSER_RUNNER: BrowserRunner = BrowserRunner::new();
}
