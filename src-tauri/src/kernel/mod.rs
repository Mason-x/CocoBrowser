//! Browser kernel abstraction layer.
//!
//! Application services (profile launch, API, MCP) depend on [`driver::KernelDriver`]
//! only. Concrete Persona kernels live behind the
//! registry. No ProfileService / UI code may assemble Chromium CLI flags.

pub mod audit;
pub mod capabilities;
pub mod cloak_license;
pub mod cloakbrowser;
pub mod downloader;
pub mod driver;
pub mod fingerprint_chromium;
pub mod geo_consistency;
pub mod install_registry;
pub mod kinds;
pub mod launch_plan;
pub mod manifest;
pub mod persona;
pub mod process_guard;
pub mod registry;
pub mod session;
pub mod update_check;

pub use audit::{AuditResult, AuditStatus, StabilityReport};
pub use capabilities::{CapabilityMode, KernelCapabilities};
pub use cloak_license::{CloakLatestRelease, CloakLicenseStatus};
pub use driver::{KernelDriver, KernelError, KernelInfo, KernelLaunchRequest};
pub use geo_consistency::{check_geo_consistency, match_persona_to_exit, GeoGateResult};
pub use install_registry::InstalledKernel;
pub use launch_plan::{AutomationMode, BrowserProcess, LaunchPlan, LocalProxyEndpoint};
pub use manifest::{KernelAsset, KernelManifest};
pub use persona::FingerprintPersona;
pub use registry::KernelRegistry;
pub use session::{SessionManager, SessionState};
pub use update_check::KernelUpdateStatus;
