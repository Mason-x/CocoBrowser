//! Temporary adapter: expose existing Wayfern launch path as a KernelDriver.
//! Behavior is intentionally unchanged — Phase 5 removes this driver.

use super::capabilities::KernelCapabilities;
use super::driver::{KernelDriver, KernelError, KernelInfo, KernelLaunchRequest};
use super::launch_plan::{AutomationMode, BrowserProcess, LaunchPlan, LocalProxyEndpoint};
use crate::browser::{create_browser, BrowserType};
use crate::wayfern_manager::WayfernManager;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct WayfernLegacyDriver;

impl WayfernLegacyDriver {
  pub fn new() -> Self {
    Self
  }

  fn install_root_for_version(version: &str) -> PathBuf {
    let mut dir = crate::app_dirs::binaries_dir();
    dir.push("wayfern");
    dir.push(version);
    dir
  }

  fn executable_for_version(version: &str) -> Result<PathBuf, KernelError> {
    let root = Self::install_root_for_version(version);
    let browser = create_browser(BrowserType::Wayfern);
    browser
      .get_executable_path(&root)
      .map_err(|e| KernelError::InvalidBinary(e.to_string()))
  }

  /// Static CLI flags shared with WayfernManager::launch_wayfern (subset used for
  /// plan inspection / tests). Dynamic flags (token, window color, extensions)
  /// are still applied inside the legacy launch path.
  pub fn base_args(
    profile_path: &str,
    cdp_port: u16,
    headless: bool,
    local_proxy: Option<&LocalProxyEndpoint>,
  ) -> Vec<String> {
    let mut args = vec![
      format!("--remote-debugging-port={cdp_port}"),
      "--remote-debugging-address=127.0.0.1".to_string(),
      format!("--user-data-dir={profile_path}"),
      "--no-first-run".to_string(),
      "--no-default-browser-check".to_string(),
      "--disable-background-mode".to_string(),
      "--disable-component-update".to_string(),
      "--disable-background-timer-throttling".to_string(),
      "--crash-server-url=".to_string(),
      "--disable-updater".to_string(),
      "--disable-session-crashed-bubble".to_string(),
      "--hide-crash-restore-bubble".to_string(),
      "--disable-infobars".to_string(),
      "--disable-features=DialMediaRouteProvider,DnsOverHttps,AsyncDns,Prefetch,PrefetchProxy,SpeculationRulesPrefetchFuture,NoStatePrefetch".to_string(),
      "--use-mock-keychain".to_string(),
      "--password-store=basic".to_string(),
    ];

    if headless {
      args.push("--headless=new".to_string());
    }

    if let Some(proxy) = local_proxy {
      // Prefer loopback proxy-server form for plan visibility. The live Wayfern
      // path still uses a PAC data URL; both point at the same sidecar.
      args.push(format!("--proxy-server={}", proxy.proxy_server_arg()));
      args.push("--proxy-bypass-list=<-loopback>".to_string());
    }

    args
  }
}

impl Default for WayfernLegacyDriver {
  fn default() -> Self {
    Self::new()
  }
}

#[async_trait]
impl KernelDriver for WayfernLegacyDriver {
  fn id(&self) -> &'static str {
    "wayfern"
  }

  fn validate_binary(&self, root: &Path) -> Result<KernelInfo, KernelError> {
    let browser = create_browser(BrowserType::Wayfern);
    let executable = browser
      .get_executable_path(root)
      .map_err(|e| KernelError::InvalidBinary(e.to_string()))?;
    if !executable.exists() {
      return Err(KernelError::InvalidBinary(format!(
        "executable missing: {}",
        executable.display()
      )));
    }
    let version = root
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string();
    Ok(KernelInfo {
      id: self.id().to_string(),
      version,
      executable,
      install_root: root.to_path_buf(),
    })
  }

  fn capabilities(&self, version: &str) -> KernelCapabilities {
    KernelCapabilities::wayfern_legacy(version)
  }

  fn build_launch_plan(&self, request: &KernelLaunchRequest) -> Result<LaunchPlan, KernelError> {
    let version = request.profile.version.clone();
    let executable = Self::executable_for_version(&version)?;
    let cdp_port = request.remote_debugging_port.unwrap_or(0);
    let profile_path = request.profile_path.to_string_lossy().to_string();
    let headless = request.headless || matches!(request.automation, AutomationMode::Headless);

    let args = Self::base_args(
      &profile_path,
      cdp_port,
      headless,
      request.local_proxy.as_ref(),
    );

    let plan = LaunchPlan {
      kernel_id: self.id().to_string(),
      kernel_version: version,
      executable,
      args,
      env: BTreeMap::new(),
      working_dir: None,
      user_data_dir: request.profile_path.clone(),
      local_proxy: request.local_proxy.clone(),
      cdp_port: if cdp_port == 0 { None } else { Some(cdp_port) },
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
    app_handle: &tauri::AppHandle,
    request: KernelLaunchRequest,
  ) -> Result<BrowserProcess, KernelError> {
    let config = request.wayfern_config.unwrap_or_default();
    let profile_path = request.profile_path.to_string_lossy().to_string();
    let proxy_url = request
      .proxy_url
      .or_else(|| request.local_proxy.as_ref().map(|p| p.proxy_server_arg()));

    let result = WayfernManager::instance()
      .launch_wayfern(
        app_handle,
        &request.profile,
        &profile_path,
        &config,
        request.url.as_deref(),
        proxy_url.as_deref(),
        request.ephemeral,
        &request.extension_paths,
        request.remote_debugging_port,
        request.headless,
      )
      .await
      .map_err(KernelError::from)?;

    Ok(BrowserProcess {
      profile_id: request.profile.id.to_string(),
      kernel_id: self.id().to_string(),
      pid: result.processId,
      created_at: Some(SystemTime::now()),
      cdp_port: result.cdp_port,
      user_data_dir: request.profile_path,
      instance_id: Some(result.id),
      used_fingerprint: result.used_fingerprint,
    })
  }

  async fn stop(&self, process: &BrowserProcess) -> Result<(), KernelError> {
    let id = process
      .instance_id
      .as_deref()
      .ok_or_else(|| KernelError::Message("missing wayfern instance id".into()))?;
    WayfernManager::instance()
      .stop_wayfern(id)
      .await
      .map_err(KernelError::from)?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::profile::BrowserProfile;

  fn sample_request(port: u16) -> KernelLaunchRequest {
    KernelLaunchRequest {
      profile: BrowserProfile {
        id: uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
        name: "t".into(),
        browser: "wayfern".into(),
        version: "1.0.0".into(),
        proxy_id: None,
        vpn_id: None,
        launch_hook: None,
        process_id: None,
        last_launch: None,
        release_type: "stable".into(),
        wayfern_config: None,
        persona: None,
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
      },
      profile_path: PathBuf::from("C:\\profiles\\test\\profile"),
      url: None,
      local_proxy: Some(LocalProxyEndpoint::socks5_loopback(port)),
      automation: AutomationMode::Manual,
      remote_debugging_port: Some(9333),
      headless: false,
      extension_paths: vec![],
      wayfern_config: None,
      persona: None,
      proxy_url: None,
      ephemeral: false,
    }
  }

  #[test]
  fn base_args_bind_loopback_and_user_data() {
    let args = WayfernLegacyDriver::base_args(
      "C:\\data\\p",
      9333,
      false,
      Some(&LocalProxyEndpoint::socks5_loopback(18080)),
    );
    assert!(args
      .iter()
      .any(|a| a == "--remote-debugging-address=127.0.0.1"));
    assert!(args.iter().any(|a| a.contains("--user-data-dir=")));
    assert!(args
      .iter()
      .any(|a| a == "--proxy-server=socks5://127.0.0.1:18080"));
    assert!(!args.iter().any(|a| a.contains('@')));
  }

  #[test]
  fn capabilities_mark_headless_experimental() {
    let caps = WayfernLegacyDriver.capabilities("149.0.0");
    assert_eq!(caps.kernel_id, "wayfern");
    assert_eq!(
      caps.headless,
      super::super::capabilities::CapabilityMode::Experimental
    );
  }

  #[test]
  fn build_launch_plan_requires_installed_binary() {
    // Without a real binary under binaries/wayfern/1.0.0 this should fail
    // validation — proves plan build goes through executable resolution.
    let driver = WayfernLegacyDriver;
    let req = sample_request(18080);
    let err = driver.build_launch_plan(&req).unwrap_err();
    match err {
      KernelError::InvalidBinary(_) | KernelError::Message(_) => {}
      other => panic!("unexpected error: {other}"),
    }
  }
}
