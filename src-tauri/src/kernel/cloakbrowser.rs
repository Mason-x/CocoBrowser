//! CloakBrowser KernelDriver for the public v146 and licensed latest-v150 modes.

use super::capabilities::KernelCapabilities;
use super::driver::{KernelDriver, KernelError, KernelInfo, KernelLaunchRequest};
use super::install_registry::{find_executable, install_root};
use super::kinds::{requires_cloak_license, CLOAK_BROWSER_146, CLOAK_BROWSER_150};
use super::launch_plan::{AutomationMode, BrowserProcess, LaunchPlan, LocalProxyEndpoint};
use super::manifest::{current_platform_id, KernelManifest};
use super::persona::{ensure_persona, FingerprintPersona, SpoofingSurface, WebRtcPolicy};
use super::process_guard::ProcessGuard;
use super::session::{SessionManager, SessionState};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

struct CloakGuard {
  kernel_id: &'static str,
  guard: ProcessGuard,
}

#[derive(Default)]
struct LiveState {
  guards: BTreeMap<String, CloakGuard>,
  latest_launch_reserved: bool,
}

lazy_static::lazy_static! {
  static ref LIVE_STATE: Mutex<LiveState> = Mutex::new(LiveState::default());
}

pub struct CloakBrowserDriver {
  id: &'static str,
}

impl CloakBrowserDriver {
  pub fn legacy_146() -> Self {
    Self {
      id: CLOAK_BROWSER_146,
    }
  }

  pub fn latest_150() -> Self {
    Self {
      id: CLOAK_BROWSER_150,
    }
  }

  fn validate_at(&self, root: &Path) -> Result<KernelInfo, KernelError> {
    let platform = current_platform_id();
    let candidates = KernelManifest::embedded()
      .ok()
      .and_then(|manifest| {
        manifest
          .kernels
          .into_iter()
          .find(|asset| asset.id == self.id && asset.platform == platform)
          .map(|asset| asset.executable_candidates)
      })
      .unwrap_or_else(|| super::downloader::cloak_executable_candidates(platform));
    let executable = find_executable(root, &candidates).ok_or_else(|| {
      KernelError::InvalidBinary(format!(
        "{} executable not found under {}",
        self.id,
        root.display()
      ))
    })?;
    let version = root
      .file_name()
      .and_then(|value| value.to_str())
      .unwrap_or("unknown")
      .to_string();
    Ok(KernelInfo {
      id: self.id.to_string(),
      version,
      executable,
      install_root: root.to_path_buf(),
    })
  }

  fn resolve_executable(&self, version: &str) -> Result<PathBuf, KernelError> {
    let root = install_root(self.id, version);
    let info = self.validate_at(&root)?;
    // Chromium exits immediately when it cannot build its sandbox, so a host
    // that forbids unprivileged user namespaces gets told why instead of
    // watching a window fail to appear.
    if super::linux_sandbox::check_install(&root) != super::linux_sandbox::SandboxReadiness::Ready {
      return Err(KernelError::Message(
        serde_json::json!({ "code": "LINUX_SANDBOX_UNAVAILABLE" }).to_string(),
      ));
    }
    Ok(info.executable)
  }

  #[allow(clippy::too_many_arguments)]
  pub fn build_args(
    persona: &FingerprintPersona,
    user_data_dir: &Path,
    local_proxy: Option<&LocalProxyEndpoint>,
    automation: AutomationMode,
    cdp_port: Option<u16>,
    extension_paths: &[String],
    url: Option<&str>,
  ) -> Result<Vec<String>, KernelError> {
    // Personas are validated against the host before this runs, so the only
    // rejection left here is a platform the kernel has no flag for.
    let platform = persona
      .platform
      .as_cli()
      .ok_or_else(|| KernelError::Unsupported("unsupported platform for CloakBrowser".into()))?;

    let mut args = vec![
      format!("--user-data-dir={}", user_data_dir.display()),
      "--no-first-run".into(),
      "--no-default-browser-check".into(),
      "--ignore-gpu-blocklist".into(),
      format!("--fingerprint={}", persona.seed),
      format!("--fingerprint-platform={platform}"),
      format!("--fingerprint-brand={}", persona.brand.as_cli()),
      format!("--fingerprint-brand-version={}", persona.brand_version),
      format!("--lang={}", persona.language),
      format!("--accept-lang={}", persona.accept_languages.join(",")),
      format!("--fingerprint-locale={}", persona.language),
      format!("--fingerprint-timezone={}", persona.timezone),
      format!(
        "--window-size={},{}",
        persona.window_width, persona.window_height
      ),
      format!("--fingerprint-screen-width={}", persona.window_width),
      format!("--fingerprint-screen-height={}", persona.window_height),
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
      args.push("--allow-profile-downgrade".into());
    }
    if persona.webrtc_policy.restricts_direct_udp() {
      args.push("--disable-non-proxied-udp".into());
    }
    if persona.webrtc_policy == WebRtcPolicy::Replace {
      // CloakBrowser resolves this through the configured proxy and replaces
      // ICE candidate IPs in the native WebRTC implementation.
      args.push("--fingerprint-webrtc-ip=auto".into());
    }
    if persona.webrtc_policy == WebRtcPolicy::Disabled {
      args.push("--disable-webrtc".into());
    }
    if let Some(version) = &persona.platform_version {
      args.push(format!("--fingerprint-platform-version={version}"));
    }
    if let Some(cores) = persona.hardware_concurrency {
      args.push(format!("--fingerprint-hardware-concurrency={cores}"));
    }
    // CloakBrowser exposes one global noise switch rather than independent
    // Canvas/WebGL switches. Treat the BitBrowser-style "both off" choice as
    // the explicit request to disable its fingerprint noise layer.
    if persona.spoofing_disabled.contains(&SpoofingSurface::Canvas)
      && persona.spoofing_disabled.contains(&SpoofingSurface::Webgl)
    {
      args.push("--fingerprint-noise=false".into());
    }
    if let Some(proxy) = local_proxy {
      args.push(format!("--proxy-server={}", proxy.proxy_server_arg()));
      args.push("--proxy-bypass-list=<-loopback>".into());
    }
    match automation {
      AutomationMode::Manual => {}
      AutomationMode::HeadedAutomation => {
        let port = cdp_port.ok_or_else(|| {
          KernelError::Message("headed automation requires a loopback CDP port".into())
        })?;
        args.push(format!("--remote-debugging-port={port}"));
        args.push("--remote-debugging-address=127.0.0.1".into());
      }
      AutomationMode::Headless => {
        let port = cdp_port.ok_or_else(|| {
          KernelError::Message("headless automation requires a loopback CDP port".into())
        })?;
        args.push("--headless=new".into());
        args.push(format!("--remote-debugging-port={port}"));
        args.push("--remote-debugging-address=127.0.0.1".into());
      }
    }
    if !extension_paths.is_empty() {
      let joined = extension_paths.join(",");
      args.push(format!("--load-extension={joined}"));
      args.push(format!("--disable-extensions-except={joined}"));
    }
    if let Some(url) = url {
      super::fingerprint_chromium::validate_launch_url(url)?;
      args.push(url.to_string());
    }
    Ok(args)
  }

  fn license_exit_error(code: i32) -> Option<KernelError> {
    let error_code = match code {
      76 => "CLOAK_SESSION_LIMIT_REACHED",
      77 => "CLOAK_LICENSE_INVALID",
      78 => "CLOAK_LICENSE_SERVER_UNAVAILABLE",
      79 => "CLOAK_LICENSE_STORAGE_FAILED",
      _ => return None,
    };
    Some(KernelError::Message(
      serde_json::json!({ "code": error_code }).to_string(),
    ))
  }
}

fn find_free_loopback_port() -> Result<u16, KernelError> {
  let listener = TcpListener::bind("127.0.0.1:0")
    .map_err(|e| KernelError::Message(format!("bind free port: {e}")))?;
  listener
    .local_addr()
    .map(|address| address.port())
    .map_err(|e| KernelError::Message(e.to_string()))
}

async fn wait_for_cdp(port: u16, timeout: Duration) -> Result<(), KernelError> {
  let client = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(1))
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

#[async_trait]
impl KernelDriver for CloakBrowserDriver {
  fn id(&self) -> &'static str {
    self.id
  }

  fn validate_binary(&self, root: &Path) -> Result<KernelInfo, KernelError> {
    self.validate_at(root)
  }

  fn capabilities(&self, version: &str) -> KernelCapabilities {
    KernelCapabilities::cloak_browser(self.id, version)
  }

  fn build_launch_plan(&self, request: &KernelLaunchRequest) -> Result<LaunchPlan, KernelError> {
    let version = request.profile.version.clone();
    let executable = self.resolve_executable(&version)?;
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
      AutomationMode::Manual => None,
    };
    let args = Self::build_args(
      &persona,
      &request.profile_path,
      request.local_proxy.as_ref(),
      request.automation,
      cdp_port,
      &request.extension_paths,
      request.url.as_deref(),
    )?;
    let mut env = BTreeMap::new();
    if requires_cloak_license(self.id) {
      let key = super::cloak_license::load_license_key()
        .map_err(|detail| {
          KernelError::Message(
            serde_json::json!({
              "code": "CLOAK_LICENSE_STORAGE_FAILED",
              "params": { "detail": detail }
            })
            .to_string(),
          )
        })?
        .ok_or_else(|| {
          KernelError::Message(
            serde_json::json!({ "code": "CLOAK_LICENSE_KEY_REQUIRED" }).to_string(),
          )
        })?;
      env.insert("CLOAKBROWSER_LICENSE_KEY".into(), key);
    }
    let plan = LaunchPlan {
      kernel_id: self.id.to_string(),
      kernel_version: version,
      executable,
      args,
      env,
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

    let result = async {
      sessions
        .set_state(profile_id, SessionState::ValidatingPersona)
        .map_err(KernelError::Message)?;
      let plan = self.build_launch_plan(&request)?;

      {
        let mut state = LIVE_STATE
          .lock()
          .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?;
        state.guards.retain(|_, entry| entry.guard.is_alive());
        if state.guards.contains_key(&profile_id.to_string()) {
          return Err(KernelError::Message(
            serde_json::json!({ "code": "PROFILE_RUNNING" }).to_string(),
          ));
        }
        if self.id == CLOAK_BROWSER_150
          && (state.latest_launch_reserved
            || state
              .guards
              .values()
              .any(|entry| entry.kernel_id == CLOAK_BROWSER_150))
        {
          return Err(KernelError::Message(
            serde_json::json!({ "code": "CLOAK_SESSION_LIMIT_REACHED" }).to_string(),
          ));
        }
        if self.id == CLOAK_BROWSER_150 {
          state.latest_launch_reserved = true;
        }
      }

      sessions
        .set_state(profile_id, SessionState::Launching)
        .map_err(KernelError::Message)?;
      let mut guard = ProcessGuard::spawn_with_env(&plan.executable, &plan.args, &plan.env)
        .map_err(KernelError::Message)?;
      tokio::time::sleep(Duration::from_millis(500)).await;
      match guard.try_exit_code().map_err(KernelError::Message)? {
        None => {}
        Some(code) => {
          return Err(Self::license_exit_error(code).unwrap_or_else(|| {
            KernelError::Message(format!("browser process exited with code {code}"))
          }));
        }
      }
      if let Some(port) = plan.cdp_port {
        wait_for_cdp(port, Duration::from_secs(15)).await?;
      }

      let job_token = guard.job_token();
      let process = BrowserProcess {
        profile_id: profile_id.to_string(),
        kernel_id: self.id.to_string(),
        pid: Some(guard.pid),
        created_at: Some(guard.created_at),
        cdp_port: plan.cdp_port,
        user_data_dir: plan.user_data_dir,
        instance_id: Some(format!("cloak-{}", guard.pid)),
        used_fingerprint: None,
      };
      sessions
        .mark_running(profile_id, process.clone(), job_token)
        .map_err(KernelError::Message)?;
      let mut state = LIVE_STATE
        .lock()
        .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?;
      state.latest_launch_reserved = false;
      state.guards.insert(
        profile_id.to_string(),
        CloakGuard {
          kernel_id: self.id,
          guard,
        },
      );
      Ok(process)
    }
    .await;

    if let Err(error) = &result {
      if self.id == CLOAK_BROWSER_150 {
        if let Ok(mut state) = LIVE_STATE.lock() {
          state.latest_launch_reserved = false;
        }
      }
      let _ = sessions.set_error(profile_id, error.to_string());
      sessions.end(profile_id);
    }
    result
  }

  async fn stop(&self, process: &BrowserProcess) -> Result<(), KernelError> {
    if let Ok(profile_id) = uuid::Uuid::parse_str(&process.profile_id) {
      let _ = SessionManager::instance().set_state(profile_id, SessionState::Stopping);
    }
    let mut state = LIVE_STATE
      .lock()
      .map_err(|_| KernelError::Message("live guards lock poisoned".into()))?;
    let entry = state.guards.remove(&process.profile_id);
    if process.kernel_id == CLOAK_BROWSER_150 {
      state.latest_launch_reserved = false;
    }
    drop(state);
    if let Some(entry) = entry {
      entry.guard.terminate().map_err(KernelError::Message)?;
    }
    if let Ok(profile_id) = uuid::Uuid::parse_str(&process.profile_id) {
      SessionManager::instance().end(profile_id);
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::kernel::persona::{BrowserBrand, FingerprintPlatform, WebRtcPolicy};
  use std::collections::BTreeSet;

  fn persona() -> FingerprintPersona {
    FingerprintPersona {
      schema_version: 1,
      seed: 42,
      platform: FingerprintPlatform::Windows,
      platform_version: Some("15.0.0".into()),
      brand: BrowserBrand::Chrome,
      brand_version: "150".into(),
      language: "en-US".into(),
      accept_languages: vec!["en-US".into(), "en".into()],
      timezone: "America/New_York".into(),
      timezone_follows_ip: true,
      language_follows_ip: false,
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
  fn uses_cloak_native_fingerprint_flags() {
    let args = CloakBrowserDriver::build_args(
      &persona(),
      Path::new("C:/profiles/a"),
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
    )
    .unwrap();
    assert!(args.iter().any(|arg| arg == "--fingerprint=42"));
    assert!(args
      .iter()
      .any(|arg| arg == "--fingerprint-timezone=America/New_York"));
    assert!(args
      .iter()
      .any(|arg| arg == "--fingerprint-screen-width=1920"));
    assert!(args.iter().any(|arg| arg == "--fingerprint-webrtc-ip=auto"));
    assert!(!args.iter().any(|arg| arg == "--fingerprint-noise=false"));
    assert!(!args.iter().any(|arg| arg == "--allow-profile-downgrade"));
  }

  // A profile moved back onto an older kernel keeps a newer version stamp on
  // its user data dir, which Chromium refuses to open without this switch.
  #[test]
  fn allows_the_profile_downgrade_the_switch_already_confirmed() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("Last Version"), "150.0.7401.9").unwrap();
    let mut persona = persona();
    persona.brand_version = "146".into();

    let args = CloakBrowserDriver::build_args(
      &persona,
      dir.path(),
      None,
      AutomationMode::Manual,
      None,
      &[],
      None,
    )
    .unwrap();
    assert!(args.iter().any(|arg| arg == "--allow-profile-downgrade"));
  }

  #[test]
  fn webrtc_modes_emit_distinct_launch_args() {
    let build = |policy| {
      let mut configured = persona();
      configured.webrtc_policy = policy;
      CloakBrowserDriver::build_args(
        &configured,
        Path::new("C:/profiles/a"),
        None,
        AutomationMode::Manual,
        None,
        &[],
        None,
      )
      .unwrap()
    };

    let privacy = build(WebRtcPolicy::Privacy);
    assert!(privacy.iter().any(|arg| arg == "--disable-non-proxied-udp"));
    assert!(!privacy
      .iter()
      .any(|arg| arg.starts_with("--fingerprint-webrtc-ip=")));

    let allow = build(WebRtcPolicy::Allow);
    assert!(allow
      .iter()
      .any(|arg| arg == "--force-webrtc-ip-handling-policy=default"));
    assert!(!allow.iter().any(|arg| arg == "--disable-non-proxied-udp"));

    let disabled = build(WebRtcPolicy::Disabled);
    assert!(disabled.iter().any(|arg| arg == "--disable-webrtc"));
  }

  #[test]
  fn maps_vendor_license_exit_codes() {
    assert!(CloakBrowserDriver::license_exit_error(76)
      .unwrap()
      .to_string()
      .contains("CLOAK_SESSION_LIMIT_REACHED"));
    assert!(CloakBrowserDriver::license_exit_error(1).is_none());
  }
}
