//! Launch plan types produced by [`super::driver::KernelDriver::build_launch_plan`].
//!
//! Business code (ProfileService / BrowserRunner / API) must not assemble
//! Chromium CLI flags directly — only a KernelDriver may produce a LaunchPlan.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// How the session should expose automation endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
  /// Manual UI launch: no remote debugging unless kernel requires internal CDP.
  #[default]
  Manual,
  /// Headed automation with loopback CDP on a random port.
  HeadedAutomation,
  /// Headless — high risk; default-disabled at product level.
  Headless,
}

/// Local proxy sidecar endpoint the browser must use (loopback only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalProxyEndpoint {
  pub host: String,
  pub port: u16,
  /// `socks5` or `http` — credentials never appear on the browser CLI.
  pub protocol: String,
}

impl LocalProxyEndpoint {
  pub fn socks5_loopback(port: u16) -> Self {
    Self {
      host: "127.0.0.1".to_string(),
      port,
      protocol: "socks5".to_string(),
    }
  }

  pub fn proxy_server_arg(&self) -> String {
    format!("{}://{}:{}", self.protocol, self.host, self.port)
  }
}

/// Fully resolved process launch plan (executable + argv + env).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchPlan {
  pub kernel_id: String,
  pub kernel_version: String,
  pub executable: PathBuf,
  pub args: Vec<String>,
  /// Child-only secrets such as a CloakBrowser key. Never serialize them into
  /// diagnostics, IPC payloads, or persisted audit output.
  #[serde(skip_serializing)]
  pub env: BTreeMap<String, String>,
  pub working_dir: Option<PathBuf>,
  pub user_data_dir: PathBuf,
  pub local_proxy: Option<LocalProxyEndpoint>,
  pub cdp_port: Option<u16>,
  pub automation: AutomationMode,
  /// Profile id this plan was built for (audit / process tracking).
  pub profile_id: String,
}

impl LaunchPlan {
  /// True if any arg looks like it embeds proxy credentials (forbidden).
  pub fn has_proxy_credentials_in_args(&self) -> bool {
    self.args.iter().any(|a| {
      let lower = a.to_ascii_lowercase();
      // crude but effective denylist for user:pass@host patterns in args
      lower.contains("://")
        && lower.contains('@')
        && (lower.contains("proxy") || lower.contains("socks") || lower.contains("http"))
        || lower.contains("--proxy-server=") && lower.contains('@')
    })
  }

  /// True if remote debugging is bound to non-loopback (forbidden).
  pub fn has_non_loopback_debugging(&self) -> bool {
    self.args.iter().any(|a| {
      let lower = a.to_ascii_lowercase();
      lower.contains("remote-debugging-address=")
        && !lower.contains("127.0.0.1")
        && !lower.contains("localhost")
        && !lower.contains("[::1]")
    })
  }
}

/// Running browser process identity (PID alone is insufficient — PID reuse).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProcess {
  pub profile_id: String,
  pub kernel_id: String,
  pub pid: Option<u32>,
  /// Process creation time when available (Windows / *nix).
  #[serde(skip)]
  pub created_at: Option<SystemTime>,
  pub cdp_port: Option<u16>,
  pub user_data_dir: PathBuf,
  /// Kernel-specific opaque instance id (e.g. Wayfern instance key).
  pub instance_id: Option<String>,
  /// Wayfern may return an upgraded fingerprint after CDP setFingerprint.
  /// Other kernels leave this `None`.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub used_fingerprint: Option<String>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn detects_credentials_in_proxy_server_arg() {
    let plan = LaunchPlan {
      kernel_id: "test".into(),
      kernel_version: "1".into(),
      executable: PathBuf::from("chrome"),
      args: vec!["--proxy-server=http://user:secret@1.2.3.4:8080".into()],
      env: BTreeMap::new(),
      working_dir: None,
      user_data_dir: PathBuf::from("/tmp/p"),
      local_proxy: None,
      cdp_port: None,
      automation: AutomationMode::Manual,
      profile_id: "p1".into(),
    };
    assert!(plan.has_proxy_credentials_in_args());
  }

  #[test]
  fn loopback_proxy_without_credentials_is_ok() {
    let plan = LaunchPlan {
      kernel_id: "test".into(),
      kernel_version: "1".into(),
      executable: PathBuf::from("chrome"),
      args: vec!["--proxy-server=socks5://127.0.0.1:12345".into()],
      env: BTreeMap::new(),
      working_dir: None,
      user_data_dir: PathBuf::from("/tmp/p"),
      local_proxy: Some(LocalProxyEndpoint::socks5_loopback(12345)),
      cdp_port: None,
      automation: AutomationMode::Manual,
      profile_id: "p1".into(),
    };
    assert!(!plan.has_proxy_credentials_in_args());
  }

  #[test]
  fn detects_non_loopback_debugging() {
    let plan = LaunchPlan {
      kernel_id: "test".into(),
      kernel_version: "1".into(),
      executable: PathBuf::from("chrome"),
      args: vec!["--remote-debugging-address=0.0.0.0".into()],
      env: BTreeMap::new(),
      working_dir: None,
      user_data_dir: PathBuf::from("/tmp/p"),
      local_proxy: None,
      cdp_port: Some(9222),
      automation: AutomationMode::HeadedAutomation,
      profile_id: "p1".into(),
    };
    assert!(plan.has_non_loopback_debugging());
  }
}
