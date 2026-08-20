//! fingerprint-chromium KernelDriver — Persona launch args + process guard.

use super::capabilities::KernelCapabilities;
use super::driver::{KernelDriver, KernelError, KernelInfo, KernelLaunchRequest};
use super::install_registry::{find_executable, install_root};
use super::launch_plan::{AutomationMode, BrowserProcess, LaunchPlan, LocalProxyEndpoint};
use super::manifest::KernelManifest;
use super::persona::{ensure_persona, FingerprintPersona, WebRtcPolicy};
use super::process_guard::ProcessGuard;
use super::session::{SessionManager, SessionState};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

// Live process guards keyed by profile id (string).
lazy_static::lazy_static! {
  static ref LIVE_GUARDS: Mutex<BTreeMap<String, ProcessGuard>> = Mutex::new(BTreeMap::new());
}

pub struct FingerprintChromiumDriver;

impl FingerprintChromiumDriver {
  pub fn new() -> Self {
    Self
  }

  fn resolve_executable(version: &str) -> Result<(PathBuf, PathBuf), KernelError> {
    let root = install_root("fingerprint-chromium", version);
    let info = Self::validate_at(&root)?;
    Ok((info.executable, root))
  }

  fn validate_at(root: &Path) -> Result<KernelInfo, KernelError> {
    let candidates = KernelManifest::embedded()
      .ok()
      .and_then(|m| {
        m.kernels
          .into_iter()
          .find(|k| {
            k.id == "fingerprint-chromium" && k.platform == super::manifest::current_platform_id()
          })
          .map(|k| k.executable_candidates)
      })
      .unwrap_or_else(|| {
        vec![
          "chrome.exe".into(),
          "Chromium/Application/chrome.exe".into(),
        ]
      });
    let executable = find_executable(root, &candidates).ok_or_else(|| {
      KernelError::InvalidBinary(format!(
        "fingerprint-chromium executable not found under {}",
        root.display()
      ))
    })?;
    let version = root
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string();
    Ok(KernelInfo {
      id: "fingerprint-chromium".to_string(),
      version,
      executable,
      install_root: root.to_path_buf(),
    })
  }

  /// Build CLI args for fingerprint-chromium 148+ (plan §6).
  #[allow(clippy::too_many_arguments)]
  pub fn build_args(
    persona: &FingerprintPersona,
    user_data_dir: &Path,
    local_proxy: Option<&LocalProxyEndpoint>,
    automation: AutomationMode,
    cdp_port: Option<u16>,
    extension_paths: &[String],
    url: Option<&str>,
    extra_safe_args: &[String],
  ) -> Result<Vec<String>, KernelError> {
    let platform = persona
      .platform
      .as_cli()
      .ok_or_else(|| KernelError::Message("unsupported platform for CLI".into()))?;

    let mut args = vec![
      format!("--user-data-dir={}", user_data_dir.display()),
      "--no-first-run".to_string(),
      "--no-default-browser-check".to_string(),
      // Profiles are stopped by killing their Job Object, which Chromium can only
      // read as a crash, so it offers to restore the previous session on the next
      // launch. That prompt is noise here — the session is restored from the
      // profile directory either way — and dismissing it on every open is worse
      // than not showing it.
      "--hide-crash-restore-bubble".to_string(),
      format!("--fingerprint={}", persona.seed),
      format!("--fingerprint-platform={platform}"),
      format!("--fingerprint-brand={}", persona.brand.as_cli()),
      format!("--fingerprint-brand-version={}", persona.brand_version),
      format!("--lang={}", persona.language),
      format!("--accept-lang={}", persona.accept_languages.join(",")),
      format!("--timezone={}", persona.timezone),
      format!(
        "--window-size={},{}",
        persona.window_width, persona.window_height
      ),
      format!(
        "--webrtc-ip-handling-policy={}",
        persona.webrtc_policy.as_cli()
      ),
      format!(
        "--force-webrtc-ip-handling-policy={}",
        persona.webrtc_policy.as_cli()
      ),
    ];

    // Chromium refuses a user data dir stamped by a newer build. A profile
    // moved onto an older kernel is warned about at switch time, so let the
    // launch that follows through instead of failing into a native dialog.
    if super::profile_data::is_downgrade(user_data_dir, &persona.brand_version) {
      args.push("--allow-profile-downgrade".to_string());
    }
    if persona.webrtc_policy.restricts_direct_udp() {
      args.push("--disable-non-proxied-udp".to_string());
    }
    if persona.webrtc_policy == WebRtcPolicy::Disabled {
      // Supported by fingerprint Chromium builds that expose the desktop
      // switch. The bundled extension is the cross-version enforcement layer.
      args.push("--disable-webrtc".to_string());
    }

    if let Some(ref pv) = persona.platform_version {
      args.push(format!("--fingerprint-platform-version={pv}"));
    }
    if let Some(cores) = persona.hardware_concurrency {
      args.push(format!("--fingerprint-hardware-concurrency={cores}"));
    }

    if !persona.spoofing_disabled.is_empty() {
      let list: Vec<&str> = persona
        .spoofing_disabled
        .iter()
        .map(|s| s.as_cli())
        .collect();
      args.push(format!("--disable-spoofing={}", list.join(",")));
    }

    if let Some(proxy) = local_proxy {
      // Credentials never appear — only loopback sidecar.
      args.push(format!("--proxy-server={}", proxy.proxy_server_arg()));
      // `<-loopback>` *removes* Chromium's implicit localhost bypass, so loopback
      // traffic goes through the proxy too and a page can neither reach local
      // services nor detect that localhost is exempt. It is also why the workbench
      // page is a file:// and not a local HTTP server — see `crate::workbench`.
      args.push("--proxy-bypass-list=<-loopback>".to_string());
    }

    match automation {
      AutomationMode::Manual => {
        // No remote debugging for normal manual launches.
      }
      AutomationMode::HeadedAutomation => {
        let port = cdp_port.ok_or_else(|| {
          KernelError::Message("headed automation requires a loopback CDP port".into())
        })?;
        args.push(format!("--remote-debugging-port={port}"));
        args.push("--remote-debugging-address=127.0.0.1".to_string());
      }
      AutomationMode::Headless => {
        let port = cdp_port.ok_or_else(|| {
          KernelError::Message("headless automation requires a loopback CDP port".into())
        })?;
        args.push("--headless=new".to_string());
        args.push(format!("--remote-debugging-port={port}"));
        args.push("--remote-debugging-address=127.0.0.1".to_string());
      }
    }

    if !extension_paths.is_empty() {
      args.push(format!("--load-extension={}", extension_paths.join(",")));
    }

    // Deny list for raw extra args.
    for a in extra_safe_args {
      deny_raw_arg(a)?;
      args.push(a.clone());
    }

    if let Some(u) = url {
      validate_launch_url(u)?;
      args.push(u.to_string());
    }

    Ok(args)
  }
}

pub(crate) fn validate_launch_url(value: &str) -> Result<(), KernelError> {
  if value.starts_with('-') {
    return Err(KernelError::Message(
      "launch URL must not be interpreted as a browser switch".into(),
    ));
  }
  let parsed =
    url::Url::parse(value).map_err(|e| KernelError::Message(format!("invalid launch URL: {e}")))?;
  if !matches!(parsed.scheme(), "http" | "https" | "about") {
    return Err(KernelError::Message(format!(
      "launch URL scheme '{}' is not allowed",
      parsed.scheme()
    )));
  }
  Ok(())
}

fn deny_raw_arg(arg: &str) -> Result<(), KernelError> {
  let lower = arg.to_ascii_lowercase();
  const DENY: &[&str] = &[
    "--no-sandbox",
    "--disable-web-security",
    "--remote-debugging-address=0.0.0.0",
    "--remote-debugging-address=::",
    "--gpu-vendor-id",
    "--gpu-device-id",
    "--fingerprint-gpu",
  ];
  for d in DENY {
    if lower.contains(d) {
      return Err(KernelError::Message(format!(
        "raw arg denied by policy: {arg}"
      )));
    }
  }
  if lower.contains("proxy-server=") && lower.contains('@') {
    return Err(KernelError::Message(
      "proxy credentials must not appear in args".into(),
    ));
  }
  Ok(())
}

fn find_free_loopback_port() -> Result<u16, KernelError> {
  let listener = TcpListener::bind("127.0.0.1:0")
    .map_err(|e| KernelError::Message(format!("bind free port: {e}")))?;
  Ok(
    listener
      .local_addr()
      .map_err(|e| KernelError::Message(e.to_string()))?
      .port(),
  )
}

impl Default for FingerprintChromiumDriver {
  fn default() -> Self {
    Self::new()
  }
}

#[async_trait]
impl KernelDriver for FingerprintChromiumDriver {
  fn id(&self) -> &'static str {
    "fingerprint-chromium"
  }

  fn validate_binary(&self, root: &Path) -> Result<KernelInfo, KernelError> {
    Self::validate_at(root)
  }

  fn capabilities(&self, version: &str) -> KernelCapabilities {
    if version.starts_with("148.") {
      KernelCapabilities::fingerprint_chromium_148()
    } else {
      KernelCapabilities::conservative(self.id(), version)
    }
  }

  fn build_launch_plan(&self, request: &KernelLaunchRequest) -> Result<LaunchPlan, KernelError> {
    let version = request.profile.version.clone();
    let (executable, _) = Self::resolve_executable(&version)?;

    let persona = ensure_persona(
      request
        .persona
        .as_ref()
        .or(request.profile.persona.as_ref()),
      &version,
    )
    .map_err(KernelError::Message)?;

    let cdp_port = match request.automation {
      AutomationMode::HeadedAutomation | AutomationMode::Headless => Some(
        request
          .remote_debugging_port
          .map(Ok)
          .unwrap_or_else(find_free_loopback_port)?,
      ),
      _ => None,
    };

    let args = Self::build_args(
      &persona,
      &request.profile_path,
      request.local_proxy.as_ref(),
      request.automation,
      cdp_port,
      &request.extension_paths,
      request.url.as_deref(),
      &[],
    )?;

    let plan = LaunchPlan {
      kernel_id: self.id().to_string(),
      kernel_version: version,
      executable,
      args,
      env: BTreeMap::new(),
      working_dir: None,
      user_data_dir: request.profile_path.clone(),
      local_proxy: request.local_proxy.clone(),
      cdp_port,
      automation: request.automation,
      profile_id: request.profile.id.to_string(),
    };

    if plan.has_proxy_credentials_in_args() {
      return Err(KernelError::Message(
        "proxy credentials must not appear in launch args".into(),
      ));
    }
    if plan.has_non_loopback_debugging() {
      return Err(KernelError::Message(
        "remote debugging must bind to loopback only".into(),
      ));
    }
    Ok(plan)
  }

  async fn launch(
    &self,
    _app_handle: &tauri::AppHandle,
    request: KernelLaunchRequest,
  ) -> Result<BrowserProcess, KernelError> {
    let profile_id = request.profile.id;
    let sessions = SessionManager::instance();

    sessions
      .try_begin_launch(profile_id)
      .map_err(KernelError::Message)?;

    let launch_result = async {
      sessions
        .set_state(profile_id, SessionState::ValidatingPersona)
        .map_err(KernelError::Message)?;

      let version = request.profile.version.clone();
      let persona = ensure_persona(
        request
          .persona
          .as_ref()
          .or(request.profile.persona.as_ref()),
        &version,
      )
      .map_err(KernelError::Message)?;

      sessions
        .set_state(profile_id, SessionState::Launching)
        .map_err(KernelError::Message)?;

      let plan = self.build_launch_plan(&KernelLaunchRequest {
        persona: Some(persona),
        ..request.clone()
      })?;

      // Double-check single instance via live guards.
      {
        let mut guards = LIVE_GUARDS
          .lock()
          .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?;
        // Closing the browser window never calls `stop()`, so a guard for a
        // process that has already exited stays in the map and refuses every
        // later launch. Ask the guard whether its process is actually alive
        // rather than trusting the map's presence.
        let key = profile_id.to_string();
        if guards.get_mut(&key).is_some_and(|guard| !guard.is_alive()) {
          log::info!("Dropping the stale process guard for profile {profile_id}");
          guards.remove(&key);
        }
        if guards.contains_key(&key) {
          return Err(KernelError::Message(format!(
            "profile {profile_id} already has a live process guard"
          )));
        }
      }

      sessions
        .set_state(profile_id, SessionState::WaitingForReady)
        .map_err(KernelError::Message)?;

      let mut guard =
        ProcessGuard::spawn(&plan.executable, &plan.args).map_err(KernelError::Message)?;

      // Ready probe: process still alive after a short settle.
      tokio::time::sleep(Duration::from_millis(400)).await;
      if !guard.is_alive() {
        return Err(KernelError::Message(
          "browser process exited immediately after launch".into(),
        ));
      }

      // Optional CDP readiness for automation mode.
      if let Some(port) = plan.cdp_port {
        wait_for_cdp(port, Duration::from_secs(15)).await?;
      }

      let job_token = guard.job_token();
      let process = BrowserProcess {
        profile_id: profile_id.to_string(),
        kernel_id: self.id().to_string(),
        pid: Some(guard.pid),
        created_at: Some(guard.created_at),
        cdp_port: plan.cdp_port,
        user_data_dir: plan.user_data_dir.clone(),
        instance_id: Some(format!("fchromium-{}", guard.pid)),
        used_fingerprint: None,
      };

      LIVE_GUARDS
        .lock()
        .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?
        .insert(profile_id.to_string(), guard);

      sessions
        .mark_running(profile_id, process.clone(), job_token)
        .map_err(KernelError::Message)?;

      Ok(process)
    }
    .await;

    if let Err(ref e) = launch_result {
      let _ = sessions.set_error(profile_id, e.to_string());
      sessions.end(profile_id);
    }

    launch_result
  }

  async fn stop(&self, process: &BrowserProcess) -> Result<(), KernelError> {
    let profile_id = process.profile_id.clone();
    let sessions = SessionManager::instance();
    if let Ok(uuid) = uuid::Uuid::parse_str(&profile_id) {
      let _ = sessions.set_state(uuid, SessionState::Stopping);
    }

    let guard = LIVE_GUARDS
      .lock()
      .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?
      .remove(&profile_id);

    if let Some(guard) = guard {
      // Verify pid still matches what we track (PID reuse protection).
      if let (Some(expected), Some(created)) = (process.pid, process.created_at) {
        if guard.pid != expected {
          log::warn!(
            "process guard pid {} != tracked {}; still terminating our job tree only",
            guard.pid,
            expected
          );
        }
        // created_at on guard should be close; we don't kill unrelated pids.
        let _ = created;
      }
      guard.terminate().map_err(KernelError::Message)?;
    } else if let Some(pid) = process.pid {
      // Fallback: only if we still own a session for this pid (no name-based kill).
      log::warn!(
        "no live guard for profile {}; cannot safely kill pid {pid} without job ownership",
        profile_id
      );
    }

    if let Ok(uuid) = uuid::Uuid::parse_str(&profile_id) {
      sessions.end(uuid);
    }
    Ok(())
  }
}

async fn wait_for_cdp(port: u16, timeout: Duration) -> Result<(), KernelError> {
  let client = reqwest::Client::builder()
    .no_proxy()
    .timeout(Duration::from_secs(2))
    .build()
    .map_err(|e| KernelError::Message(e.to_string()))?;
  let url = format!("http://127.0.0.1:{port}/json/version");
  let start = std::time::Instant::now();
  loop {
    if start.elapsed() > timeout {
      return Err(KernelError::Message(format!(
        "CDP not ready on 127.0.0.1:{port} within {timeout:?}"
      )));
    }
    if client.get(&url).send().await.is_ok() {
      return Ok(());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::kernel::persona::{BrowserBrand, FingerprintPlatform, WebRtcPolicy};
  use crate::profile::BrowserProfile;
  use std::collections::BTreeSet;

  fn sample_persona() -> FingerprintPersona {
    FingerprintPersona {
      schema_version: 1,
      seed: 42,
      platform: FingerprintPlatform::Windows,
      platform_version: Some("15.0.0".into()),
      brand: BrowserBrand::Chrome,
      brand_version: "148".into(),
      language: "en-US".into(),
      accept_languages: vec!["en-US".into(), "en".into()],
      timezone: "America/New_York".into(),
      timezone_follows_ip: true,
      language_follows_ip: true,
      hardware_concurrency: Some(8),
      window_width: 1920,
      window_height: 1080,
      webrtc_policy: WebRtcPolicy::Replace,
      spoofing_disabled: BTreeSet::new(),
      proxy_geo_signature: None,
      capability_revision: "test".into(),
    }
  }

  #[test]
  fn launch_args_stable_for_same_persona() {
    let p = sample_persona();
    let ud = PathBuf::from("C:\\profiles\\a\\profile");
    let a1 = FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &[],
    )
    .unwrap();
    let a2 = FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &[],
    )
    .unwrap();
    assert_eq!(a1, a2);
    assert!(a1.iter().any(|x| x == "--fingerprint=42"));
    assert!(a1.iter().any(|x| x == "--fingerprint-platform=windows"));
    assert!(a1.iter().any(|x| x == "--fingerprint-brand-version=148"));
    assert!(a1.iter().any(|x| x == "--timezone=America/New_York"));
    assert!(a1.iter().any(|x| x == "--disable-non-proxied-udp"));
    assert!(!a1.iter().any(|x| x.contains("remote-debugging")));
    assert!(!a1.iter().any(|x| x.contains("no-sandbox")));
  }

  #[test]
  fn allow_mode_does_not_force_udp_through_the_proxy() {
    let mut persona = sample_persona();
    persona.webrtc_policy = WebRtcPolicy::Allow;
    let args = FingerprintChromiumDriver::build_args(
      &persona,
      Path::new("C:/profiles/a"),
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &[],
    )
    .unwrap();
    assert!(args
      .iter()
      .any(|arg| arg == "--force-webrtc-ip-handling-policy=default"));
    assert!(!args.iter().any(|arg| arg == "--disable-non-proxied-udp"));
  }

  #[test]
  fn different_seeds_produce_different_args() {
    let mut p1 = sample_persona();
    let mut p2 = sample_persona();
    p1.seed = 1;
    p2.seed = 2;
    let ud = PathBuf::from("/tmp/p");
    let a1 = FingerprintChromiumDriver::build_args(
      &p1,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &[],
    )
    .unwrap();
    let a2 = FingerprintChromiumDriver::build_args(
      &p2,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &[],
    )
    .unwrap();
    assert_ne!(a1, a2);
  }

  #[test]
  fn automation_binds_loopback_only() {
    let p = sample_persona();
    let ud = PathBuf::from("/tmp/p");
    let args = FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      Some(&LocalProxyEndpoint::socks5_loopback(18080)),
      AutomationMode::HeadedAutomation,
      Some(9333),
      &[],
      None,
      &[],
    )
    .unwrap();
    assert!(args
      .iter()
      .any(|a| a == "--remote-debugging-address=127.0.0.1"));
    assert!(args
      .iter()
      .any(|a| a == "--proxy-server=socks5://127.0.0.1:18080"));
    assert!(!args.iter().any(|a| a.contains('@')));
  }

  #[test]
  fn headless_and_denied_args_rejected() {
    let p = sample_persona();
    let ud = PathBuf::from("/tmp/p");
    assert!(FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Headless,
      None,
      &[],
      None,
      &[],
    )
    .is_err());

    let headless = FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Headless,
      Some(9444),
      &[],
      Some("https://example.com"),
      &[],
    )
    .unwrap();
    assert!(headless.iter().any(|arg| arg == "--headless=new"));
    assert!(headless
      .iter()
      .any(|arg| arg == "--remote-debugging-address=127.0.0.1"));
    assert!(FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
      &["--no-sandbox".into()],
    )
    .is_err());
  }

  #[test]
  fn browser_internal_schemes_stay_blocked() {
    // The workbench page is opened by the extension itself, so none of these
    // need to be reachable from a launch URL.
    for url in [
      "chrome://settings/",
      "chrome-extension://abcdefghijklmnop/workbench.html",
      "file:///C:/secret.txt",
      "devtools://devtools/bundled/inspector.html",
    ] {
      assert!(validate_launch_url(url).is_err(), "{url} must stay blocked");
    }
  }

  #[test]
  fn launch_url_cannot_inject_switches_or_local_files() {
    let p = sample_persona();
    let ud = PathBuf::from("/tmp/p");
    for url in ["--no-sandbox", "file:///C:/secret.txt", "not a url"] {
      assert!(FingerprintChromiumDriver::build_args(
        &p,
        &ud,
        None,
        AutomationMode::Manual,
        None,
        &[],
        Some(url),
        &[],
      )
      .is_err());
    }
    assert!(FingerprintChromiumDriver::build_args(
      &p,
      &ud,
      None,
      AutomationMode::Manual,
      None,
      &[],
      Some("about:blank"),
      &[],
    )
    .is_ok());
  }

  #[test]
  fn capabilities_for_148() {
    let d = FingerprintChromiumDriver;
    let caps = d.capabilities("148.0.7778.215");
    assert_eq!(caps.kernel_id, "fingerprint-chromium");
    assert_eq!(
      caps.cross_os,
      super::super::capabilities::CapabilityMode::Unsupported
    );
  }

  #[test]
  fn sample_profile_request_shape() {
    let _p = BrowserProfile {
      id: uuid::Uuid::nil(),
      name: "t".into(),
      browser: "fingerprint-chromium".into(),
      version: "148.0.7778.215".into(),
      proxy_id: None,
      vpn_id: None,
      launch_hook: None,
      process_id: None,
      last_launch: None,
      release_type: "stable".into(),
      persona: Some(sample_persona()),
      group_id: None,
      tags: vec![],
      note: None,
      window_color: None,
      sync_mode: Default::default(),
      encryption_salt: None,
      last_sync: None,
      host_os: Some("windows".into()),
      ephemeral: false,
      extension_group_id: None,
      proxy_bypass_rules: vec![],
      created_by_id: None,
      created_by_email: None,
      dns_blocklist: None,
      password_protected: false,
      created_at: None,
      updated_at: None,
    };
  }
}
