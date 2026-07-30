//! Per-kernel capability matrix for fingerprint / launch options.
//!
//! Unknown or future kernel versions default to the conservative matrix:
//! seed, same-OS identity, language, timezone, window, WebRTC only.

use serde::{Deserialize, Serialize};

/// How a fingerprint surface is exposed for a given kernel version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityMode {
  /// User (or Persona) may set this field explicitly.
  Configurable,
  /// Controlled only by the seed / kernel; no precise UI input.
  SeedDriven,
  /// Not available for this kernel.
  Unsupported,
  /// Available but not verified; requires explicit user confirmation.
  Experimental,
}

/// Capability matrix for one kernel version range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KernelCapabilities {
  pub kernel_id: String,
  pub min_version: String,
  pub max_version: Option<String>,
  pub seed: CapabilityMode,
  pub identity: CapabilityMode,
  pub locale: CapabilityMode,
  pub timezone: CapabilityMode,
  pub hardware_concurrency: CapabilityMode,
  pub canvas: CapabilityMode,
  pub audio: CapabilityMode,
  pub fonts: CapabilityMode,
  pub client_rects: CapabilityMode,
  pub gpu: CapabilityMode,
  pub custom_gpu_metadata: CapabilityMode,
  pub geolocation: CapabilityMode,
  pub cross_os: CapabilityMode,
  pub headless: CapabilityMode,
}

impl KernelCapabilities {
  /// Conservative defaults for unknown kernels / versions.
  pub fn conservative(kernel_id: impl Into<String>, min_version: impl Into<String>) -> Self {
    Self {
      kernel_id: kernel_id.into(),
      min_version: min_version.into(),
      max_version: None,
      seed: CapabilityMode::Configurable,
      identity: CapabilityMode::Configurable,
      locale: CapabilityMode::Configurable,
      timezone: CapabilityMode::Configurable,
      hardware_concurrency: CapabilityMode::Configurable,
      canvas: CapabilityMode::SeedDriven,
      audio: CapabilityMode::SeedDriven,
      fonts: CapabilityMode::SeedDriven,
      client_rects: CapabilityMode::SeedDriven,
      gpu: CapabilityMode::SeedDriven,
      custom_gpu_metadata: CapabilityMode::Unsupported,
      geolocation: CapabilityMode::Configurable,
      cross_os: CapabilityMode::Unsupported,
      headless: CapabilityMode::Unsupported,
    }
  }

  /// Wayfern legacy matrix (pre-migration). CDP fingerprint APIs are experimental
  /// from this driver's point of view; cross-OS remains gated externally today.
  pub fn wayfern_legacy(version: &str) -> Self {
    Self {
      kernel_id: "wayfern".to_string(),
      min_version: version.to_string(),
      max_version: None,
      seed: CapabilityMode::Experimental,
      identity: CapabilityMode::Configurable,
      locale: CapabilityMode::Configurable,
      timezone: CapabilityMode::Configurable,
      hardware_concurrency: CapabilityMode::Configurable,
      canvas: CapabilityMode::Configurable,
      audio: CapabilityMode::Configurable,
      fonts: CapabilityMode::Configurable,
      client_rects: CapabilityMode::Configurable,
      gpu: CapabilityMode::Configurable,
      custom_gpu_metadata: CapabilityMode::Configurable,
      geolocation: CapabilityMode::Configurable,
      cross_os: CapabilityMode::Experimental,
      headless: CapabilityMode::Experimental,
    }
  }

  /// fingerprint-chromium 148 fixed matrix (Phase 2+).
  pub fn fingerprint_chromium_148() -> Self {
    Self {
      kernel_id: "fingerprint-chromium".to_string(),
      min_version: "148.0.0.0".to_string(),
      max_version: Some("148.999.999.999".to_string()),
      seed: CapabilityMode::Configurable,
      identity: CapabilityMode::Configurable,
      locale: CapabilityMode::Configurable,
      timezone: CapabilityMode::Configurable,
      hardware_concurrency: CapabilityMode::Configurable,
      canvas: CapabilityMode::SeedDriven,
      audio: CapabilityMode::SeedDriven,
      fonts: CapabilityMode::SeedDriven,
      client_rects: CapabilityMode::SeedDriven,
      gpu: CapabilityMode::SeedDriven,
      custom_gpu_metadata: CapabilityMode::Unsupported,
      geolocation: CapabilityMode::Configurable,
      cross_os: CapabilityMode::Unsupported,
      // Upstream only normalizes the UA in headless; other headless signals
      // remain detectable, so expose it solely behind an explicit opt-in.
      headless: CapabilityMode::Experimental,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn conservative_disables_cross_os_and_headless() {
    let caps = KernelCapabilities::conservative("unknown", "0.0.0");
    assert_eq!(caps.cross_os, CapabilityMode::Unsupported);
    assert_eq!(caps.headless, CapabilityMode::Unsupported);
    assert_eq!(caps.custom_gpu_metadata, CapabilityMode::Unsupported);
  }

  #[test]
  fn fchromium_148_marks_canvas_seed_driven() {
    let caps = KernelCapabilities::fingerprint_chromium_148();
    assert_eq!(caps.canvas, CapabilityMode::SeedDriven);
    assert_eq!(caps.seed, CapabilityMode::Configurable);
    assert_eq!(caps.custom_gpu_metadata, CapabilityMode::Unsupported);
    assert_eq!(caps.headless, CapabilityMode::Experimental);
  }
}
