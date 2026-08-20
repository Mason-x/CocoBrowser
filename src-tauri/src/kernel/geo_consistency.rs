//! Exit-IP / timezone / language consistency for per-profile proxies.
//!
//! Before launching a Persona-based fingerprint kernel:
//! 1. Query public IP through the same local proxy sidecar the browser will use
//! 2. Resolve country + IANA timezone via local GeoLite (no remote geo API required)
//! 3. Compare with Persona; block on hard mismatches unless user matches exit
//!
//! `proxy_geo_signature` never contains passwords — only type/host/port and a
//! username hash plus observed exit IP / country / timezone.

use crate::browser::ProxySettings;
use crate::geoip_downloader::GeoIPDownloader;
use crate::geolocation::{self, Geolocation};
use crate::ip_utils;
use crate::kernel::launch_plan::LocalProxyEndpoint;
use crate::kernel::persona::FingerprintPersona;
use crate::proxy_manager::{CLOUD_PROXY_ID, PROXY_MANAGER};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExitObservation {
  pub exit_ip: String,
  pub country_code: Option<String>,
  pub timezone: String,
  pub language: String,
  pub accept_languages: Vec<String>,
  pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GeoGateResult {
  /// No proxy / geo gate skipped (direct) or persona already matches signature.
  Pass {
    observation: Option<ExitObservation>,
  },
  /// Exit reachable but persona timezone (or stamped signature) disagrees.
  Blocked {
    observation: ExitObservation,
    reason: String,
  },
  /// Upstream proxy rejected / IP fetch failed — do not launch.
  ProxyFailed { reason: String },
  /// Profile uses cloud-managed proxy which local-first builds refuse.
  CloudProxyRejected,
}

/// Hash username for signature (never log or store the raw password).
pub fn username_hash(username: Option<&str>) -> String {
  let raw = username.unwrap_or("");
  let mut hasher = Sha256::new();
  hasher.update(raw.as_bytes());
  let digest = hasher.finalize();
  let mut hex = String::with_capacity(16);
  for b in digest.iter().take(8) {
    use std::fmt::Write;
    let _ = write!(hex, "{b:02x}");
  }
  hex
}

/// Stable routing signature (no password). Plan §7.8.
pub fn proxy_geo_signature(
  proxy: Option<&ProxySettings>,
  vpn_id: Option<&str>,
  exit_ip: Option<&str>,
  country: Option<&str>,
  timezone: Option<&str>,
) -> String {
  let routing = if let Some(id) = vpn_id {
    format!("vpn:{id}")
  } else if let Some(p) = proxy {
    format!(
      "proxy:{}://{}:{}#u={}",
      p.proxy_type.to_lowercase(),
      p.host,
      p.port,
      username_hash(p.username.as_deref())
    )
  } else {
    "direct".to_string()
  };
  format!(
    "v1:{routing}|ip={}|cc={}|tz={}",
    exit_ip.unwrap_or(""),
    country.unwrap_or(""),
    timezone.unwrap_or("")
  )
}

/// Reject Coco cloud-managed proxy IDs for local-first kernels.
pub fn reject_cloud_proxy_id(proxy_id: Option<&str>) -> Result<(), String> {
  match proxy_id {
    Some(id)
      if id == CLOUD_PROXY_ID
        || id.starts_with("cloud-")
        || PROXY_MANAGER.is_cloud_or_derived(id) => Err(
      "Cloud-managed proxies are not supported in local fingerprint mode. Use a self-hosted HTTP/SOCKS proxy."
        .into(),
    ),
    _ => Ok(()),
  }
}

/// Redact secrets from a proxy URL before logging.
pub fn redact_proxy_url(url: &str) -> String {
  // scheme://user:pass@host:port → scheme://***@host:port
  if let Some(scheme_end) = url.find("://") {
    let rest = &url[scheme_end + 3..];
    if let Some(at) = rest.find('@') {
      let hostpart = &rest[at + 1..];
      return format!("{}://***@{}", &url[..scheme_end], hostpart);
    }
  }
  url.to_string()
}

fn geo_from_ip(ip: &str) -> Result<Geolocation, String> {
  geolocation::get_geolocation(ip).map_err(|e| e.to_string())
}

fn geolocation_failure_result(
  exit_ip: &str,
  error: &str,
  database_available: bool,
) -> GeoGateResult {
  if !database_available {
    log::warn!(
      "GeoIP database is unavailable; proxy exit {exit_ip} is reachable but timezone verification is deferred: {error}"
    );
    return GeoGateResult::Pass { observation: None };
  }

  GeoGateResult::ProxyFailed {
    reason: format!("geolocation lookup failed for exit {exit_ip}: {error}"),
  }
}

/// Build observation from exit IP + GeoLite (+ locale selector).
pub fn observation_from_exit_ip(
  exit_ip: &str,
  proxy: Option<&ProxySettings>,
  vpn_id: Option<&str>,
) -> Result<ExitObservation, String> {
  let geo = geo_from_ip(exit_ip)?;
  let language = geo.locale.as_string();
  let mut accept = vec![language.clone()];
  if let Some(lang_only) = language.split('-').next() {
    if lang_only != language {
      accept.push(lang_only.to_string());
    }
  }
  let country = geo.locale.region.clone();
  let signature = proxy_geo_signature(
    proxy,
    vpn_id,
    Some(exit_ip),
    country.as_deref(),
    Some(&geo.timezone),
  );
  Ok(ExitObservation {
    exit_ip: exit_ip.to_string(),
    country_code: country,
    timezone: geo.timezone,
    language,
    accept_languages: accept,
    signature,
  })
}

/// Apply exit observation onto persona (user chose “match to current exit”).
/// Align every field the user has left following the exit.
///
/// Fields with their follow flag turned off are left exactly as the user set
/// them — that is the whole point of turning the flag off. The signature is
/// stamped either way so the next launch through the same exit is cheap.
pub fn match_persona_to_exit(persona: &mut FingerprintPersona, obs: &ExitObservation) {
  if persona.timezone_follows_ip {
    persona.timezone = obs.timezone.clone();
  }
  if persona.language_follows_ip {
    persona.language = obs.language.clone();
    persona.accept_languages = obs.accept_languages.clone();
  }
  persona.proxy_geo_signature = Some(obs.signature.clone());
}

/// Unconditional variant for the explicit "match to exit" button, which the user
/// pressed precisely to overwrite whatever is there.
pub fn force_match_persona_to_exit(persona: &mut FingerprintPersona, obs: &ExitObservation) {
  persona.timezone = obs.timezone.clone();
  persona.language = obs.language.clone();
  persona.accept_languages = obs.accept_languages.clone();
  persona.proxy_geo_signature = Some(obs.signature.clone());
}

/// Evaluate gate given an already-observed exit (unit-test friendly).
/// When `require_match` is true, timezone mismatch always blocks.
pub fn evaluate_gate(
  persona: &FingerprintPersona,
  observation: Option<ExitObservation>,
  require_match: bool,
) -> GeoGateResult {
  let Some(obs) = observation else {
    return GeoGateResult::Pass { observation: None };
  };

  // Stamped signature matches current routing+exit → pass.
  if persona.proxy_geo_signature.as_deref() == Some(obs.signature.as_str()) {
    return GeoGateResult::Pass {
      observation: Some(obs),
    };
  }

  if persona.timezone != obs.timezone {
    // Following the exit: adapt rather than refuse. Proxies are not freely
    // interchangeable — you use the exit you have — and a persona claiming one
    // timezone while its IP geolocates to another is a far stronger correlation
    // signal than a timezone that follows its exit, which is just what a
    // traveller or VPN user looks like. The caller applies the match before it
    // builds the launch plan.
    if persona.timezone_follows_ip {
      return GeoGateResult::Pass {
        observation: Some(obs),
      };
    }
    // Pinned deliberately. `require_match` decides whether that is allowed to
    // launch: the gate still exists for callers that want it (the audit path),
    // but a user who pinned a timezone on purpose is not second-guessed here.
    if require_match {
      return GeoGateResult::Blocked {
        reason: format!(
          "Persona timezone '{}' does not match proxy exit timezone '{}'. Match persona to exit or change proxy.",
          persona.timezone, obs.timezone
        ),
        observation: obs,
      };
    }
    return GeoGateResult::Pass {
      observation: Some(obs),
    };
  }

  // Timezone matches but signature not stamped yet — pass and allow caller to stamp.
  GeoGateResult::Pass {
    observation: Some(obs),
  }
}

/// Match a profile's persona to the current proxy exit (starts a short-lived
/// local sidecar check). Used by UI "match to exit" actions.
#[tauri::command]
pub async fn match_profile_persona_to_exit(
  app_handle: tauri::AppHandle,
  profile_id: String,
) -> Result<crate::profile::BrowserProfile, String> {
  use crate::profile::ProfileManager;
  use crate::proxy_manager::PROXY_MANAGER;

  reject_cloud_proxy_id(
    ProfileManager::instance()
      .list_profiles()
      .map_err(|e| e.to_string())?
      .iter()
      .find(|p| p.id.to_string() == profile_id)
      .and_then(|p| p.proxy_id.as_deref()),
  )?;

  let profiles = ProfileManager::instance()
    .list_profiles()
    .map_err(|e| e.to_string())?;
  let mut profile = profiles
    .into_iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| format!("profile not found: {profile_id}"))?;

  let mut persona =
    crate::kernel::persona::ensure_persona(profile.persona.as_ref(), &profile.version)?;

  if !GeoIPDownloader::is_geoip_database_available() {
    GeoIPDownloader::instance()
      .download_geoip_database(&app_handle)
      .await
      .map_err(|error| format!("GeoIP database download failed: {error}"))?;
    if !GeoIPDownloader::is_geoip_database_available() {
      return Err(
        "GeoIP database is still downloading. Wait a moment and try Match to exit again."
          .to_string(),
      );
    }
  }

  let upstream = profile
    .proxy_id
    .as_ref()
    .and_then(|id| PROXY_MANAGER.get_proxy_settings_by_id(id));

  // Temporary sidecar for exit check only.
  let upstream_url = upstream.as_ref().map(|p| {
    // Build URL without logging. Password stays in memory for the worker only.
    crate::proxy_manager::ProxyManager::build_proxy_url(p)
  });
  let worker = crate::proxy_runner::start_proxy_process(upstream_url, None)
    .await
    .map_err(|e| e.to_string())?;
  let port = worker.local_port.unwrap_or(0);
  let local = LocalProxyEndpoint {
    host: "127.0.0.1".into(),
    port,
    protocol: "http".into(),
  };

  let gate = check_geo_consistency(
    &persona,
    Some(&local),
    upstream.as_ref(),
    profile.vpn_id.as_deref(),
    profile.proxy_id.as_deref(),
  )
  .await;
  let _ = crate::proxy_runner::stop_proxy_process(&worker.id).await;

  match gate {
    GeoGateResult::Pass {
      observation: Some(obs),
    }
    | GeoGateResult::Blocked {
      observation: obs, ..
    } => {
      // The user pressed "match to exit", so overwrite even fields whose follow
      // flag is off — that is what the button is for.
      force_match_persona_to_exit(&mut persona, &obs);
      profile.persona = Some(persona);
      ProfileManager::instance()
        .save_profile(&profile)
        .map_err(|e| e.to_string())?;
      Ok(profile)
    }
    GeoGateResult::Pass { observation: None } => Err("No exit observation".into()),
    GeoGateResult::ProxyFailed { reason } => Err(reason),
    GeoGateResult::CloudProxyRejected => Err("Cloud-managed proxies are not supported".into()),
  }
}

/// Full check: fetch IP through local sidecar, GeoLite lookup, evaluate.
pub async fn check_geo_consistency(
  persona: &FingerprintPersona,
  local_proxy: Option<&LocalProxyEndpoint>,
  upstream: Option<&ProxySettings>,
  vpn_id: Option<&str>,
  proxy_id: Option<&str>,
) -> GeoGateResult {
  if let Err(e) = reject_cloud_proxy_id(proxy_id) {
    log::warn!("{e}");
    return GeoGateResult::CloudProxyRejected;
  }

  let Some(local) = local_proxy else {
    return GeoGateResult::ProxyFailed {
      reason: "local proxy is required for exit-IP verification".to_string(),
    };
  };

  // Always use loopback sidecar URL — never put upstream credentials on the client.
  let scheme = match local.protocol.as_str() {
    "socks5" => "socks5h",
    "http" | "https" => local.protocol.as_str(),
    other => {
      return GeoGateResult::ProxyFailed {
        reason: format!("unsupported local proxy protocol: {other}"),
      };
    }
  };
  let local_url = format!("{scheme}://{}:{}", local.host, local.port);
  log::info!(
    "Geo consistency: fetching exit IP via local proxy {}",
    redact_proxy_url(&local_url)
  );

  let exit_ip = match ip_utils::fetch_public_ip(Some(&local_url)).await {
    Ok(ip) => ip,
    Err(e) => {
      // Do not include proxy password in error path.
      let msg = e.to_string();
      let safe = if msg.to_ascii_lowercase().contains("password")
        || msg.contains("://") && msg.contains('@')
      {
        "proxy exit IP check failed (credentials or upstream error)".to_string()
      } else {
        format!("proxy exit IP check failed: {msg}")
      };
      return GeoGateResult::ProxyFailed { reason: safe };
    }
  };

  let obs = match observation_from_exit_ip(&exit_ip, upstream, vpn_id) {
    Ok(o) => o,
    Err(e) => {
      // A missing database is not a proxy failure. The exit IP was fetched
      // through the same sidecar, so allow this launch while the background
      // downloader prepares GeoLite for the next launch. Corrupt/invalid
      // databases remain blocking errors.
      return geolocation_failure_result(
        &exit_ip,
        &e,
        GeoIPDownloader::is_geoip_database_available(),
      );
    }
  };

  // `require_match: false` — a timezone disagreement adapts the persona rather
  // than refusing the launch. Unreachable exits and unusable GeoIP data still
  // fail closed above; only the persona-vs-exit disagreement is now self-healing.
  evaluate_gate(persona, Some(obs), false)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::kernel::persona::{BrowserBrand, FingerprintPlatform, WebRtcPolicy};
  use std::collections::BTreeSet;

  fn persona_tz(tz: &str) -> FingerprintPersona {
    FingerprintPersona {
      schema_version: 1,
      seed: 1,
      platform: FingerprintPlatform::Windows,
      platform_version: Some("15.0.0".into()),
      brand: BrowserBrand::Chrome,
      brand_version: "148".into(),
      language: "en-US".into(),
      accept_languages: vec!["en-US".into()],
      timezone: tz.into(),
      timezone_follows_ip: true,
      language_follows_ip: true,
      hardware_concurrency: Some(8),
      window_width: 1920,
      window_height: 1080,
      webrtc_policy: WebRtcPolicy::Replace,
      spoofing_disabled: BTreeSet::new(),
      proxy_geo_signature: None,
      capability_revision: "t".into(),
    }
  }

  /// A persona with both fields pinned by the user.
  fn persona_tz_pinned(tz: &str) -> FingerprintPersona {
    let mut p = persona_tz(tz);
    p.timezone_follows_ip = false;
    p.language_follows_ip = false;
    p
  }

  #[test]
  fn signature_excludes_password() {
    let p = ProxySettings {
      proxy_type: "socks5".into(),
      host: "1.2.3.4".into(),
      port: 1080,
      username: Some("alice".into()),
      password: Some("s3cret".into()),
    };
    let sig = proxy_geo_signature(
      Some(&p),
      None,
      Some("8.8.8.8"),
      Some("US"),
      Some("America/New_York"),
    );
    assert!(!sig.contains("s3cret"));
    assert!(!sig.contains("alice")); // username is hashed
    assert!(sig.contains(&username_hash(Some("alice"))));
  }

  #[test]
  fn redact_hides_userinfo() {
    let r = redact_proxy_url("socks5://user:pass@10.0.0.1:1080");
    assert!(!r.contains("pass"));
    assert!(r.contains("***@10.0.0.1:1080"));
  }

  #[test]
  fn missing_geoip_database_defers_timezone_gate() {
    assert_eq!(
      geolocation_failure_result("65.49.220.239", "database missing", false),
      GeoGateResult::Pass { observation: None }
    );
  }

  #[test]
  fn available_but_invalid_geoip_database_still_blocks() {
    assert!(matches!(
      geolocation_failure_result("65.49.220.239", "database corrupt", true),
      GeoGateResult::ProxyFailed { .. }
    ));
  }

  #[test]
  fn cloud_proxy_rejected() {
    assert!(reject_cloud_proxy_id(Some(CLOUD_PROXY_ID)).is_err());
    assert!(reject_cloud_proxy_id(Some("cloud-foo")).is_err());
    assert!(reject_cloud_proxy_id(Some("uuid-local")).is_ok());
  }

  fn tokyo_exit() -> ExitObservation {
    ExitObservation {
      exit_ip: "1.1.1.1".into(),
      country_code: Some("JP".into()),
      timezone: "Asia/Tokyo".into(),
      language: "ja-JP".into(),
      accept_languages: vec!["ja-JP".into(), "ja".into()],
      signature: "v1:test".into(),
    }
  }

  #[test]
  fn following_the_exit_adapts_instead_of_blocking() {
    let p = persona_tz("America/New_York");
    // The observation must come back so the caller can apply the match; a Pass
    // without one would silently launch the mismatched persona.
    match evaluate_gate(&p, Some(tokyo_exit()), true) {
      GeoGateResult::Pass {
        observation: Some(obs),
      } => assert_eq!(obs.timezone, "Asia/Tokyo"),
      other => panic!("expected Pass with an observation, got {other:?}"),
    }
  }

  #[test]
  fn a_stale_signature_no_longer_blocks_a_new_exit() {
    // Reconnecting through a different exit used to be a hard failure once the
    // profile had been stamped. While following, it must re-adapt instead.
    let mut p = persona_tz("America/New_York");
    p.proxy_geo_signature = Some("v1:some-older-exit".into());
    match evaluate_gate(&p, Some(tokyo_exit()), true) {
      GeoGateResult::Pass {
        observation: Some(obs),
      } => assert_eq!(obs.timezone, "Asia/Tokyo"),
      other => panic!("expected Pass with an observation, got {other:?}"),
    }
  }

  #[test]
  fn a_pinned_timezone_still_blocks_when_match_is_required() {
    let p = persona_tz_pinned("America/New_York");
    match evaluate_gate(&p, Some(tokyo_exit()), true) {
      GeoGateResult::Blocked { .. } => {}
      other => panic!("expected Blocked, got {other:?}"),
    }
  }

  #[test]
  fn a_pinned_timezone_launches_when_match_is_not_required() {
    let p = persona_tz_pinned("America/New_York");
    match evaluate_gate(&p, Some(tokyo_exit()), false) {
      GeoGateResult::Pass { .. } => {}
      other => panic!("expected Pass, got {other:?}"),
    }
  }

  #[test]
  fn matching_aligns_every_following_field() {
    let mut p = persona_tz("America/New_York");
    match_persona_to_exit(&mut p, &tokyo_exit());
    assert_eq!(p.timezone, "Asia/Tokyo");
    assert_eq!(p.language, "ja-JP");
    assert_eq!(p.accept_languages, vec!["ja-JP", "ja"]);
    assert_eq!(p.proxy_geo_signature.as_deref(), Some("v1:test"));
  }

  #[test]
  fn matching_leaves_pinned_fields_alone() {
    let mut p = persona_tz_pinned("America/New_York");
    match_persona_to_exit(&mut p, &tokyo_exit());
    assert_eq!(p.timezone, "America/New_York");
    assert_eq!(p.language, "en-US");
    assert_eq!(p.accept_languages, vec!["en-US"]);
    // Still stamped: the profile did launch through this exit, and the stamp is
    // what makes the next launch through it cheap.
    assert_eq!(p.proxy_geo_signature.as_deref(), Some("v1:test"));
  }

  #[test]
  fn each_follow_flag_acts_independently() {
    let mut p = persona_tz("America/New_York");
    p.language_follows_ip = false;
    match_persona_to_exit(&mut p, &tokyo_exit());
    assert_eq!(p.timezone, "Asia/Tokyo", "timezone still follows");
    assert_eq!(p.language, "en-US", "language was pinned");
  }

  #[test]
  fn the_match_to_exit_button_overrides_pinned_fields() {
    let mut p = persona_tz_pinned("America/New_York");
    force_match_persona_to_exit(&mut p, &tokyo_exit());
    assert_eq!(p.timezone, "Asia/Tokyo");
    assert_eq!(p.language, "ja-JP");
  }

  #[test]
  fn personas_saved_before_the_flags_existed_follow_only_the_timezone() {
    // Existing profiles on disk have no such keys. Timezone must deserialise as
    // following — reading it as pinned would silently reintroduce the launch
    // block this change removes. Language is the opposite: it gates nothing, and
    // a stored language is a choice, so absence means pinned.
    let json = r#"{
      "schemaVersion": 1, "seed": 1, "platform": "windows",
      "platformVersion": "15.0.0", "brand": "chrome", "brandVersion": "148",
      "language": "en-US", "acceptLanguages": ["en-US"],
      "timezone": "America/New_York", "hardwareConcurrency": 8,
      "windowWidth": 1920, "windowHeight": 1080,
      "webrtcPolicy": "disable_non_proxied_udp", "capabilityRevision": "t"
    }"#;
    let p: FingerprintPersona = serde_json::from_str(json).expect("legacy persona must parse");
    assert!(p.timezone_follows_ip);
    assert!(!p.language_follows_ip);
  }

  #[test]
  fn matching_signature_passes() {
    let mut p = persona_tz("Asia/Tokyo");
    let obs = ExitObservation {
      exit_ip: "1.1.1.1".into(),
      country_code: Some("JP".into()),
      timezone: "Asia/Tokyo".into(),
      language: "ja-JP".into(),
      accept_languages: vec!["ja-JP".into()],
      signature: "v1:same".into(),
    };
    p.proxy_geo_signature = Some("v1:same".into());
    match evaluate_gate(&p, Some(obs), true) {
      GeoGateResult::Pass { .. } => {}
      other => panic!("expected Pass, got {other:?}"),
    }
  }

  #[test]
  fn match_persona_updates_fields() {
    let mut p = persona_tz("America/New_York");
    let obs = ExitObservation {
      exit_ip: "9.9.9.9".into(),
      country_code: Some("DE".into()),
      timezone: "Europe/Berlin".into(),
      language: "de-DE".into(),
      accept_languages: vec!["de-DE".into(), "de".into()],
      signature: "v1:de".into(),
    };
    match_persona_to_exit(&mut p, &obs);
    assert_eq!(p.timezone, "Europe/Berlin");
    assert_eq!(p.language, "de-DE");
    assert_eq!(p.proxy_geo_signature.as_deref(), Some("v1:de"));
  }

  #[test]
  fn username_hash_stable() {
    assert_eq!(username_hash(Some("bob")), username_hash(Some("bob")));
    assert_ne!(username_hash(Some("bob")), username_hash(Some("alice")));
  }
}
