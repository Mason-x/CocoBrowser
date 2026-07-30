//! KernelDriver trait — single entry for validate / capabilities / launch plan / launch.

use super::capabilities::KernelCapabilities;
use super::launch_plan::{AutomationMode, BrowserProcess, LaunchPlan, LocalProxyEndpoint};
use super::persona::FingerprintPersona;
use crate::profile::BrowserProfile;
use crate::wayfern_manager::WayfernConfig;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelInfo {
  pub id: String,
  pub version: String,
  pub executable: PathBuf,
  pub install_root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
  #[error("{0}")]
  Message(String),
  #[error("kernel not found: {0}")]
  NotFound(String),
  #[error("invalid binary: {0}")]
  InvalidBinary(String),
  #[error("unsupported: {0}")]
  Unsupported(String),
}

impl From<String> for KernelError {
  fn from(s: String) -> Self {
    KernelError::Message(s)
  }
}

impl From<&str> for KernelError {
  fn from(s: &str) -> Self {
    KernelError::Message(s.to_string())
  }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for KernelError {
  fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
    KernelError::Message(e.to_string())
  }
}

/// Everything needed to build a launch plan or launch a profile session.
/// Kernel-specific optional fields live here so Profile/API code stays free of
/// CLI flag assembly.
#[derive(Debug, Clone)]
pub struct KernelLaunchRequest {
  pub profile: BrowserProfile,
  pub profile_path: PathBuf,
  pub url: Option<String>,
  pub local_proxy: Option<LocalProxyEndpoint>,
  pub automation: AutomationMode,
  pub remote_debugging_port: Option<u16>,
  pub headless: bool,
  pub extension_paths: Vec<String>,
  /// Temporary until Wayfern is removed (Phase 5).
  pub wayfern_config: Option<WayfernConfig>,
  /// Stable fingerprint identity for fingerprint-chromium (and future kernels).
  pub persona: Option<FingerprintPersona>,
  /// Pre-formatted local proxy URL for kernels that still take a single string
  /// (legacy Wayfern path). Prefer `local_proxy` for new kernels.
  pub proxy_url: Option<String>,
  pub ephemeral: bool,
}

#[async_trait]
pub trait KernelDriver: Send + Sync {
  fn id(&self) -> &'static str;

  fn validate_binary(&self, root: &Path) -> Result<KernelInfo, KernelError>;

  fn capabilities(&self, version: &str) -> KernelCapabilities;

  /// Build a complete launch plan. Must not spawn processes.
  fn build_launch_plan(&self, request: &KernelLaunchRequest) -> Result<LaunchPlan, KernelError>;

  /// Spawn the browser for this plan / request. Implementations may perform
  /// post-launch kernel-specific setup (e.g. legacy Wayfern CDP fingerprint).
  async fn launch(
    &self,
    app_handle: &tauri::AppHandle,
    request: KernelLaunchRequest,
  ) -> Result<BrowserProcess, KernelError>;

  async fn stop(&self, process: &BrowserProcess) -> Result<(), KernelError>;
}
