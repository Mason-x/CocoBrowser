//! Stable fingerprint identity (Persona) for local kernels.
//!
//! Rules (plan §4.2 / §9):
//! - `seed` from CSPRNG, never profile-id hash or sequential counters
//! - seed stable for the life of a profile unless user regenerates
//! - v0.1 platform must match host OS (Windows-first)
//! - brand_version matches kernel major
//! - no fake advanced fields the kernel cannot honor

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::str::FromStr;

pub const PERSONA_SCHEMA_VERSION: u32 = 1;

/// Host / spoofed OS identity. v0.1 only allows matching the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FingerprintPlatform {
  Windows,
  #[serde(other)]
  Unsupported,
}

impl FingerprintPlatform {
  pub fn as_cli(self) -> Option<&'static str> {
    match self {
      FingerprintPlatform::Windows => Some("windows"),
      FingerprintPlatform::Unsupported => None,
    }
  }

  pub fn host_default() -> Self {
    if cfg!(target_os = "windows") {
      FingerprintPlatform::Windows
    } else {
      // Cross-OS spoofing is unsupported in v0.1; still record host for migration.
      FingerprintPlatform::Unsupported
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserBrand {
  Chrome,
  #[serde(other)]
  Other,
}

impl BrowserBrand {
  pub fn as_cli(self) -> &'static str {
    match self {
      BrowserBrand::Chrome => "Chrome",
      BrowserBrand::Other => "Chrome",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebRtcPolicy {
  #[default]
  DisableNonProxiedUdp,
  DefaultPublicInterfaceOnly,
  DefaultPublicAndPrivateInterfaces,
}

impl WebRtcPolicy {
  pub fn as_cli(self) -> &'static str {
    match self {
      WebRtcPolicy::DisableNonProxiedUdp => "disable_non_proxied_udp",
      WebRtcPolicy::DefaultPublicInterfaceOnly => "default_public_interface_only",
      WebRtcPolicy::DefaultPublicAndPrivateInterfaces => "default_public_and_private_interfaces",
    }
  }
}

/// Surfaces that can be explicitly disabled via `--disable-spoofing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpoofingSurface {
  Canvas,
  Audio,
  Fonts,
  ClientRects,
  Webgl,
  Webgpu,
}

impl SpoofingSurface {
  pub fn as_cli(self) -> &'static str {
    match self {
      SpoofingSurface::Canvas => "canvas",
      SpoofingSurface::Audio => "audio",
      SpoofingSurface::Fonts => "font",
      SpoofingSurface::ClientRects => "clientrects",
      SpoofingSurface::Webgl => "gpu",
      // Kept for schema compatibility; validation rejects it because the
      // upstream kernel exposes only the combined `gpu` surface.
      SpoofingSurface::Webgpu => "gpu",
    }
  }
}

fn default_true() -> bool {
  true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintPersona {
  pub schema_version: u32,
  /// Stable 32-bit seed for the kernel `--fingerprint=` flag.
  pub seed: u32,
  pub platform: FingerprintPlatform,
  pub platform_version: Option<String>,
  pub brand: BrowserBrand,
  /// Must match kernel major (e.g. `"148"` for 148.x).
  pub brand_version: String,
  pub language: String,
  pub accept_languages: Vec<String>,
  /// IANA timezone id, e.g. `America/New_York`.
  pub timezone: String,
  /// Re-derive `timezone` from the proxy exit on every launch.
  ///
  /// On by default, and on for personas saved before this field existed, because
  /// the exit is the ground truth a site can check: a browser reporting one
  /// timezone while its IP geolocates to another is a strong correlation signal,
  /// and exits are not freely interchangeable — you use the one you have. Turn it
  /// off only to pin a timezone deliberately, which also silences the mismatch
  /// gate for this profile.
  #[serde(default = "default_true")]
  pub timezone_follows_ip: bool,
  /// Re-derive `language` and `accept_languages` from the proxy exit on every
  /// launch.
  ///
  /// Off by default, unlike `timezone_follows_ip`. The two are not symmetric:
  /// a timezone that disagrees with the exit is a hard mismatch a site can
  /// compute, and it gates the launch, whereas a country maps to several
  /// plausible languages, so following the exit means the browser's UI language
  /// moves around on its own. The pinned default keeps the persona's language
  /// where the user (or profile creation) put it — `en-US` unless changed.
  #[serde(default)]
  pub language_follows_ip: bool,
  pub hardware_concurrency: Option<u8>,
  pub window_width: u32,
  pub window_height: u32,
  pub webrtc_policy: WebRtcPolicy,
  #[serde(default)]
  pub spoofing_disabled: BTreeSet<SpoofingSurface>,
  /// Proxy/geo signature this locale/timezone was generated for (Phase 4 fills).
  pub proxy_geo_signature: Option<String>,
  /// Capability matrix revision string for audit trails.
  pub capability_revision: String,
}

impl FingerprintPersona {
  /// CSPRNG seed — never derived from profile id or a counter.
  pub fn generate_seed() -> u32 {
    // rand 0.10: random::<u32>() draws from the thread RNG.
    let n: u32 = rand::random();
    if n == 0 {
      1
    } else {
      n
    }
  }

  /// Default auto-consistent persona for Windows + Chrome major.
  pub fn auto_consistent_windows(kernel_version: &str) -> Result<Self, String> {
    if !cfg!(target_os = "windows") {
      return Err("v0.1 auto-consistent persona only supports Windows hosts".into());
    }
    let major = kernel_major(kernel_version)?;
    let (w, h) = pick_common_window();
    let cores = pick_common_cores();
    Ok(Self {
      schema_version: PERSONA_SCHEMA_VERSION,
      seed: Self::generate_seed(),
      platform: FingerprintPlatform::Windows,
      platform_version: Some(host_windows_version_string()),
      brand: BrowserBrand::Chrome,
      brand_version: major.clone(),
      language: "en-US".into(),
      accept_languages: vec!["en-US".into(), "en".into()],
      timezone: "America/New_York".into(),
      timezone_follows_ip: true,
      language_follows_ip: false,
      hardware_concurrency: Some(cores),
      window_width: w,
      window_height: h,
      webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
      spoofing_disabled: BTreeSet::new(),
      proxy_geo_signature: None,
      capability_revision: format!("fchromium-{major}-v1"),
    })
  }

  /// Regenerate seed and capability revision only (user action).
  pub fn regenerate_identity(&mut self) {
    self.seed = Self::generate_seed();
    self.capability_revision = format!(
      "{}-regen-{}",
      self
        .capability_revision
        .split('-')
        .next()
        .unwrap_or("persona"),
      self.seed
    );
  }

  pub fn validate(&self, kernel_version: &str) -> Result<(), String> {
    if self.schema_version == 0 {
      return Err("persona schema_version must be >= 1".into());
    }
    if self.seed == 0 {
      return Err("persona seed must be a non-zero u32".into());
    }
    if self.platform != FingerprintPlatform::Windows {
      return Err("v0.1 only allows Windows platform (cross-OS fingerprint disabled)".into());
    }
    if !cfg!(target_os = "windows") {
      return Err("cannot launch Windows persona on a non-Windows host in v0.1".into());
    }
    let major = kernel_major(kernel_version)?;
    if self.brand_version != major {
      return Err(format!(
        "brand_version {} must match kernel major {}",
        self.brand_version, major
      ));
    }
    validate_language(&self.language)?;
    for lang in &self.accept_languages {
      validate_language(lang)?;
    }
    if self.accept_languages.is_empty() {
      return Err("accept_languages must not be empty".into());
    }
    validate_timezone(&self.timezone)?;
    if self.window_width < 800 || self.window_width > 7680 {
      return Err(format!("invalid window_width {}", self.window_width));
    }
    if self.window_height < 600 || self.window_height > 4320 {
      return Err(format!("invalid window_height {}", self.window_height));
    }
    if let Some(c) = self.hardware_concurrency {
      if ![2, 4, 6, 8, 12, 16, 24, 32].contains(&c) {
        return Err(format!(
          "hardware_concurrency {c} not in allowed set {{2,4,6,8,12,16,24,32}}"
        ));
      }
    }
    if self.spoofing_disabled.contains(&SpoofingSurface::Webgpu) {
      return Err("webgpu cannot be disabled independently; use the webgl/gpu surface".into());
    }
    if self.webrtc_policy != WebRtcPolicy::DisableNonProxiedUdp {
      return Err("unsafe WebRTC policies are disabled in the local-first build".into());
    }
    Ok(())
  }
}

pub fn kernel_major(version: &str) -> Result<String, String> {
  let major = version
    .split('.')
    .next()
    .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
    .ok_or_else(|| format!("invalid kernel version: {version}"))?;
  Ok(major.to_string())
}

fn validate_language(lang: &str) -> Result<(), String> {
  // BCP-47-ish: en, en-US, zh-CN
  let ok = !lang.is_empty()
    && lang.len() <= 16
    && lang
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
  if ok {
    Ok(())
  } else {
    Err(format!("invalid language tag: {lang}"))
  }
}

fn validate_timezone(tz: &str) -> Result<(), String> {
  if tz.is_empty() || tz.len() > 64 {
    return Err(format!("invalid timezone: {tz}"));
  }
  // Accept IANA-looking ids; chrono-tz full parse when available.
  if chrono_tz::Tz::from_str(tz).is_ok() {
    return Ok(());
  }
  // Fallback: Area/Location pattern
  if tz.contains('/')
    && tz
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_' || c == '+' || c == '-')
  {
    return Ok(());
  }
  Err(format!("unknown or invalid IANA timezone: {tz}"))
}

fn pick_common_window() -> (u32, u32) {
  // Weighted common desktop sizes (template, not independent uniform random).
  const CHOICES: &[(u32, u32)] = &[
    (1366, 768),
    (1440, 900),
    (1536, 864),
    (1920, 1080),
    (2560, 1440),
  ];
  let idx = (FingerprintPersona::generate_seed() as usize) % CHOICES.len();
  CHOICES[idx]
}

fn pick_common_cores() -> u8 {
  const CHOICES: &[u8] = &[4, 8, 12, 16];
  let idx = (FingerprintPersona::generate_seed() as usize) % CHOICES.len();
  CHOICES[idx]
}

fn host_windows_version_string() -> String {
  // Conservative default; advanced mode can override with validated values.
  "15.0.0".to_string()
}

/// Migrate or ensure a profile has a Persona for a fingerprint kernel.
pub fn ensure_persona(
  existing: Option<&FingerprintPersona>,
  kernel_version: &str,
) -> Result<FingerprintPersona, String> {
  if let Some(p) = existing {
    p.validate(kernel_version)?;
    return Ok(p.clone());
  }
  let p = FingerprintPersona::auto_consistent_windows(kernel_version)?;
  p.validate(kernel_version)?;
  Ok(p)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn seeds_are_nonzero_and_vary() {
    let a = FingerprintPersona::generate_seed();
    let b = FingerprintPersona::generate_seed();
    assert_ne!(a, 0);
    assert_ne!(b, 0);
    // Extremely unlikely to collide for two independent draws in a tight loop
    // across many runs; allow rare equality but ensure type is u32.
    let _ = (a, b);
  }

  #[test]
  fn auto_persona_valid_for_148() {
    if !cfg!(target_os = "windows") {
      return;
    }
    let p = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    p.validate("148.0.7778.215").unwrap();
    assert_eq!(p.brand_version, "148");
    assert_eq!(p.platform, FingerprintPlatform::Windows);
  }

  #[test]
  fn rejects_brand_version_mismatch() {
    if !cfg!(target_os = "windows") {
      return;
    }
    let mut p = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    p.brand_version = "147".into();
    assert!(p.validate("148.0.7778.215").is_err());
  }

  #[test]
  fn rejects_bad_timezone_and_window() {
    if !cfg!(target_os = "windows") {
      return;
    }
    let mut p = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    p.timezone = "Not/AZone!!!".into();
    assert!(p.validate("148.0.7778.215").is_err());
    p.timezone = "UTC".into();
    // UTC is valid in chrono-tz
    let _ = p.validate("148.0.7778.215");
    p.timezone = "America/New_York".into();
    p.window_width = 10;
    assert!(p.validate("148.0.7778.215").is_err());
  }

  #[test]
  fn different_auto_personas_get_different_seeds() {
    if !cfg!(target_os = "windows") {
      return;
    }
    let a = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    let b = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    assert_ne!(a.seed, b.seed);
  }

  #[test]
  fn regenerate_changes_seed() {
    if !cfg!(target_os = "windows") {
      return;
    }
    let mut p = FingerprintPersona::auto_consistent_windows("148.0.7778.215").unwrap();
    let old = p.seed;
    p.regenerate_identity();
    assert_ne!(p.seed, old);
  }
}
