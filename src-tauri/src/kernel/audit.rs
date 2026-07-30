//! Fingerprint consistency and leak audit (plan §10).
//!
//! Works offline for expected/persona checks. Live observation uses headed
//! Chromium + loopback CDP when a kernel binary is installed.

use super::driver::{KernelDriver, KernelLaunchRequest};
use super::fingerprint_chromium::FingerprintChromiumDriver;
use super::launch_plan::{AutomationMode, LocalProxyEndpoint};
use super::persona::{ensure_persona, FingerprintPersona};
use super::process_guard::ProcessGuard;
use super::session::SessionManager;
use crate::profile::{BrowserProfile, ProfileManager};
use crate::proxy_manager::PROXY_MANAGER;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
  Pass,
  Warning,
  Fail,
  Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuditFinding {
  pub code: String,
  pub severity: AuditStatus,
  pub message: String,
  pub expected: Option<String>,
  pub observed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ObservedFingerprint {
  pub user_agent: Option<String>,
  pub platform: Option<String>,
  pub webdriver: Option<bool>,
  pub language: Option<String>,
  pub languages: Option<Vec<String>>,
  pub timezone: Option<String>,
  pub timezone_offset: Option<i32>,
  pub hardware_concurrency: Option<u32>,
  pub device_memory: Option<f64>,
  pub max_touch_points: Option<u32>,
  pub screen_width: Option<u32>,
  pub screen_height: Option<u32>,
  pub outer_width: Option<u32>,
  pub outer_height: Option<u32>,
  pub device_pixel_ratio: Option<f64>,
  pub canvas_hash: Option<String>,
  pub audio_hash: Option<String>,
  pub webgl_vendor: Option<String>,
  pub webgl_renderer: Option<String>,
  pub webrtc_candidates: Option<Vec<String>>,
  pub brands: Option<String>,
  pub full_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuditResult {
  pub profile_id: String,
  pub kernel_version: String,
  pub observed_at: u64,
  pub expected: FingerprintPersona,
  pub observed: Option<ObservedFingerprint>,
  pub consistency_errors: Vec<AuditFinding>,
  pub leak_findings: Vec<AuditFinding>,
  pub stability_hash: String,
  pub status: AuditStatus,
  pub collection_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StabilityReport {
  pub profile_id: String,
  pub rounds: u32,
  pub hashes: Vec<String>,
  pub stable: bool,
  pub status: AuditStatus,
  pub findings: Vec<AuditFinding>,
  pub last_result: Option<AuditResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuditSummary {
  pub status: AuditStatus,
  pub observed_at: u64,
  pub stability_hash: String,
  pub finding_count: usize,
}

const COLLECT_JS: &str = r#"(async () => {
  const hashStr = (s) => {
    // Two independent 32-bit FNV-style accumulators. This works even when a
    // target disables WebCrypto and is sufficient for cross-launch drift.
    let a = 0x811c9dc5, b = 0x9e3779b9;
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      a = Math.imul(a ^ c, 0x01000193) >>> 0;
      b = Math.imul(b ^ c, 0x85ebca6b) >>> 0;
    }
    return a.toString(16).padStart(8,'0') + b.toString(16).padStart(8,'0');
  };
  let canvasHash = null;
  try {
    const c = document.createElement('canvas');
    c.width = 240; c.height = 60;
    const ctx = c.getContext('2d');
    ctx.textBaseline = 'top';
    ctx.font = '14px Arial';
    ctx.fillStyle = '#f60';
    ctx.fillRect(0,0,240,60);
    ctx.fillStyle = '#069';
    ctx.fillText('fingerprint-audit', 2, 2);
    canvasHash = hashStr(c.toDataURL());
  } catch (e) {}
  let audioHash = null;
  try {
    const ctx = new (window.OfflineAudioContext || window.webkitOfflineAudioContext)(1, 44100, 44100);
    const osc = ctx.createOscillator();
    osc.type = 'triangle';
    osc.frequency.value = 10000;
    const comp = ctx.createDynamicsCompressor();
    osc.connect(comp); comp.connect(ctx.destination); osc.start(0);
    const buf = await ctx.startRendering();
    const data = buf.getChannelData(0).slice(0, 100);
    audioHash = hashStr(Array.from(data).join(','));
  } catch (e) {}
  let webglVendor = null, webglRenderer = null;
  try {
    const c = document.createElement('canvas');
    const gl = c.getContext('webgl') || c.getContext('experimental-webgl');
    if (gl) {
      const dbg = gl.getExtension('WEBGL_debug_renderer_info');
      if (dbg) {
        webglVendor = gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL);
        webglRenderer = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
      }
    }
  } catch (e) {}
  let brands = null, fullVersion = null;
  try {
    if (navigator.userAgentData) {
      brands = JSON.stringify(navigator.userAgentData.brands || []);
      const he = await navigator.userAgentData.getHighEntropyValues(['fullVersionList','platform','platformVersion']);
      fullVersion = JSON.stringify(he.fullVersionList || he);
    }
  } catch (e) {}
  let webrtcCandidates = [];
  try {
    await new Promise((resolve) => {
      const pc = new RTCPeerConnection({iceServers:[]});
      pc.createDataChannel('x');
      pc.onicecandidate = (ev) => {
        if (ev.candidate && ev.candidate.candidate) webrtcCandidates.push(ev.candidate.candidate);
      };
      pc.createOffer().then(o => pc.setLocalDescription(o));
      setTimeout(() => { try { pc.close(); } catch(e){} resolve(); }, 800);
    });
  } catch (e) {}
  return {
    userAgent: navigator.userAgent || null,
    platform: navigator.platform || null,
    webdriver: !!navigator.webdriver,
    language: navigator.language || null,
    languages: navigator.languages ? Array.from(navigator.languages) : null,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || null,
    timezoneOffset: new Date().getTimezoneOffset(),
    hardwareConcurrency: navigator.hardwareConcurrency || null,
    deviceMemory: navigator.deviceMemory || null,
    maxTouchPoints: navigator.maxTouchPoints || null,
    screenWidth: screen.width || null,
    screenHeight: screen.height || null,
    outerWidth: window.outerWidth || null,
    outerHeight: window.outerHeight || null,
    devicePixelRatio: window.devicePixelRatio || null,
    canvasHash,
    audioHash,
    webglVendor,
    webglRenderer,
    webrtcCandidates,
    brands,
    fullVersion,
  };
})()"#;

const AUDIT_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Local fingerprint audit</title></head>
<body><main>Local fingerprint audit in progress.</main></body></html>"#;

fn now_secs() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0)
}

fn finding(
  code: &str,
  severity: AuditStatus,
  message: impl Into<String>,
  expected: Option<String>,
  observed: Option<String>,
) -> AuditFinding {
  AuditFinding {
    code: code.into(),
    severity,
    message: message.into(),
    expected,
    observed,
  }
}

/// Stability hash over expected persona + selected observed fields.
pub fn compute_stability_hash(
  persona: &FingerprintPersona,
  observed: Option<&ObservedFingerprint>,
) -> String {
  let mut hasher = Sha256::new();
  hasher.update(persona.seed.to_le_bytes());
  hasher.update(persona.brand_version.as_bytes());
  hasher.update(persona.language.as_bytes());
  hasher.update(persona.timezone.as_bytes());
  hasher.update(persona.window_width.to_le_bytes());
  hasher.update(persona.window_height.to_le_bytes());
  if let Some(c) = persona.hardware_concurrency {
    hasher.update([c]);
  }
  if let Some(o) = observed {
    if let Some(ref h) = o.canvas_hash {
      hasher.update(h.as_bytes());
    }
    if let Some(ref h) = o.audio_hash {
      hasher.update(h.as_bytes());
    }
    if let Some(ref ua) = o.user_agent {
      hasher.update(ua.as_bytes());
    }
    if let Some(ref tz) = o.timezone {
      hasher.update(tz.as_bytes());
    }
    if let Some(ref platform) = o.platform {
      hasher.update(platform.as_bytes());
    }
    if let Some(ref languages) = o.languages {
      for language in languages {
        hasher.update(language.as_bytes());
        hasher.update([0]);
      }
    }
    for value in [
      o.hardware_concurrency,
      o.max_touch_points,
      o.screen_width,
      o.screen_height,
      o.outer_width,
      o.outer_height,
    ]
    .into_iter()
    .flatten()
    {
      hasher.update(value.to_le_bytes());
    }
    for value in [o.device_memory, o.device_pixel_ratio]
      .into_iter()
      .flatten()
    {
      hasher.update(value.to_le_bytes());
    }
    for value in [
      o.webgl_vendor.as_ref(),
      o.webgl_renderer.as_ref(),
      o.brands.as_ref(),
      o.full_version.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
      hasher.update(value.as_bytes());
    }
  }
  let digest = hasher.finalize();
  let mut hex = String::with_capacity(digest.len() * 2);
  for b in digest {
    use std::fmt::Write;
    let _ = write!(hex, "{b:02x}");
  }
  hex
}

/// Compare persona expectations with optional live observation.
pub fn evaluate_audit(
  profile: &BrowserProfile,
  persona: &FingerprintPersona,
  observed: Option<ObservedFingerprint>,
  collection_mode: &str,
) -> AuditResult {
  let mut consistency = Vec::new();
  let mut leaks = Vec::new();

  // Persona self-consistency
  if let Err(e) = persona.validate(&profile.version) {
    consistency.push(finding("PERSONA_INVALID", AuditStatus::Fail, e, None, None));
  }

  if persona.platform != super::persona::FingerprintPlatform::Windows {
    consistency.push(finding(
      "CROSS_OS_UNSUPPORTED",
      AuditStatus::Fail,
      "v0.1 only allows Windows platform persona",
      Some("windows".into()),
      Some(format!("{:?}", persona.platform)),
    ));
  }

  if let Some(ref obs) = observed {
    // webdriver should be false for normal manual; for CDP automation it may be true
    if obs.webdriver == Some(true) && collection_mode == "headed_automation" {
      consistency.push(finding(
        "WEBDRIVER_TRUE_UNDER_CDP",
        AuditStatus::Warning,
        "navigator.webdriver is true under headed automation/CDP collection",
        Some("false (manual)".into()),
        Some("true".into()),
      ));
    } else if obs.webdriver == Some(true) {
      consistency.push(finding(
        "WEBDRIVER_UNEXPECTED",
        AuditStatus::Fail,
        "navigator.webdriver is true",
        Some("false".into()),
        Some("true".into()),
      ));
    }

    if let Some(ref tz) = obs.timezone {
      if tz != &persona.timezone {
        consistency.push(finding(
          "TIMEZONE_MISMATCH",
          AuditStatus::Fail,
          "Observed timezone differs from persona",
          Some(persona.timezone.clone()),
          Some(tz.clone()),
        ));
      }
    }

    if let Some(ref lang) = obs.language {
      // Accept exact match or shared language primary subtag (en-US vs en).
      let primary = persona
        .language
        .split('-')
        .next()
        .unwrap_or(&persona.language);
      let lang_primary = lang.split('-').next().unwrap_or(lang);
      if !lang.eq_ignore_ascii_case(&persona.language)
        && !lang_primary.eq_ignore_ascii_case(primary)
      {
        consistency.push(finding(
          "LANGUAGE_MISMATCH",
          AuditStatus::Warning,
          "Observed language differs from persona",
          Some(persona.language.clone()),
          Some(lang.clone()),
        ));
      }
    }

    if let Some(ref languages) = obs.languages {
      let matches = persona
        .accept_languages
        .iter()
        .enumerate()
        .all(|(index, expected)| {
          languages
            .get(index)
            .is_some_and(|observed| observed.eq_ignore_ascii_case(expected))
        });
      if !matches {
        consistency.push(finding(
          "ACCEPT_LANGUAGES_MISMATCH",
          AuditStatus::Warning,
          "navigator.languages differs from the persona order",
          Some(persona.accept_languages.join(",")),
          Some(languages.join(",")),
        ));
      }
    }

    if let Some(ref platform) = obs.platform {
      if platform != "Win32" {
        consistency.push(finding(
          "PLATFORM_MISMATCH",
          AuditStatus::Fail,
          "navigator.platform does not expose the Windows persona",
          Some("Win32".into()),
          Some(platform.clone()),
        ));
      }
    }

    if let Some(cores) = obs.hardware_concurrency {
      if let Some(expected) = persona.hardware_concurrency {
        if cores != u32::from(expected) {
          consistency.push(finding(
            "HW_CONCURRENCY_MISMATCH",
            AuditStatus::Warning,
            "hardwareConcurrency differs from persona",
            Some(expected.to_string()),
            Some(cores.to_string()),
          ));
        }
      }
    }

    if let Some(ref ua) = obs.user_agent {
      if !ua.contains(&persona.brand_version) {
        consistency.push(finding(
          "UA_BRAND_VERSION",
          AuditStatus::Fail,
          "User-Agent does not contain expected Chrome major version",
          Some(format!("Chrome/{}", persona.brand_version)),
          Some(ua.clone()),
        ));
      }
    }

    if let (Some(width), Some(height)) = (obs.outer_width, obs.outer_height) {
      let width_delta = width.abs_diff(persona.window_width);
      let height_delta = height.abs_diff(persona.window_height);
      if width_delta > 64 || height_delta > 96 {
        consistency.push(finding(
          "WINDOW_SIZE_MISMATCH",
          AuditStatus::Warning,
          "Observed outer window is materially different from the persona",
          Some(format!(
            "{}x{}",
            persona.window_width, persona.window_height
          )),
          Some(format!("{width}x{height}")),
        ));
      }
    }

    for (code, label, value) in [
      ("CANVAS_NOT_COLLECTED", "canvas", obs.canvas_hash.as_ref()),
      ("AUDIO_NOT_COLLECTED", "audio", obs.audio_hash.as_ref()),
      (
        "WEBGL_VENDOR_NOT_COLLECTED",
        "WebGL vendor",
        obs.webgl_vendor.as_ref(),
      ),
      (
        "WEBGL_RENDERER_NOT_COLLECTED",
        "WebGL renderer",
        obs.webgl_renderer.as_ref(),
      ),
    ] {
      if value.is_none() {
        consistency.push(finding(
          code,
          AuditStatus::Fail,
          format!("Live audit did not collect {label}"),
          None,
          None,
        ));
      }
    }

    if obs.device_memory.is_none() {
      consistency.push(finding(
        "DEVICE_MEMORY_NOT_COLLECTED",
        AuditStatus::Warning,
        "navigator.deviceMemory was unavailable",
        None,
        None,
      ));
    }
    if obs.brands.is_none() || obs.full_version.is_none() {
      consistency.push(finding(
        "CLIENT_HINTS_NOT_COLLECTED",
        AuditStatus::Warning,
        "User-Agent Client Hints were unavailable",
        Some(format!("Chrome/{}", persona.brand_version)),
        None,
      ));
    } else if !obs
      .full_version
      .as_deref()
      .is_some_and(|value| value.contains(&persona.brand_version))
    {
      consistency.push(finding(
        "CLIENT_HINTS_VERSION_MISMATCH",
        AuditStatus::Fail,
        "High-entropy Client Hints do not contain the kernel major",
        Some(persona.brand_version.clone()),
        obs.full_version.clone(),
      ));
    }

    // WebRTC host candidates can leak real IPs
    if let Some(ref cands) = obs.webrtc_candidates {
      for c in cands {
        if let Some(address) = exposed_webrtc_host_address(c) {
          leaks.push(finding(
            "WEBRTC_HOST_CANDIDATE",
            AuditStatus::Fail,
            "WebRTC ICE host candidate exposes an IP address",
            None,
            Some(address),
          ));
        }
      }
    }
  } else {
    consistency.push(finding(
      "NO_LIVE_OBSERVATION",
      AuditStatus::Unsupported,
      "Live observation not collected (kernel not installed or CDP unavailable)",
      None,
      None,
    ));
  }

  let status = worst_status(consistency.iter().chain(leaks.iter()).map(|f| f.severity));

  let stability_hash = compute_stability_hash(persona, observed.as_ref());

  AuditResult {
    profile_id: profile.id.to_string(),
    kernel_version: profile.version.clone(),
    observed_at: now_secs(),
    expected: persona.clone(),
    observed,
    consistency_errors: consistency,
    leak_findings: leaks,
    stability_hash,
    status,
    collection_mode: collection_mode.into(),
  }
}

fn exposed_webrtc_host_address(candidate: &str) -> Option<String> {
  let fields: Vec<&str> = candidate.split_whitespace().collect();
  let typ_index = fields.iter().position(|field| *field == "typ")?;
  if fields.get(typ_index + 1).copied() != Some("host") {
    return None;
  }
  let address = *fields.get(4)?;
  address
    .parse::<std::net::IpAddr>()
    .ok()
    .map(|_| address.to_string())
}

fn worst_status(iter: impl Iterator<Item = AuditStatus>) -> AuditStatus {
  let mut worst = AuditStatus::Pass;
  for s in iter {
    worst = match (worst, s) {
      (_, AuditStatus::Fail) => AuditStatus::Fail,
      (AuditStatus::Fail, _) => AuditStatus::Fail,
      (_, AuditStatus::Warning) => AuditStatus::Warning,
      (AuditStatus::Warning, _) => AuditStatus::Warning,
      (_, AuditStatus::Unsupported) => AuditStatus::Unsupported,
      (AuditStatus::Unsupported, _) => AuditStatus::Unsupported,
      _ => AuditStatus::Pass,
    };
  }
  worst
}

pub fn audit_path(profile_id: &str) -> PathBuf {
  crate::app_dirs::profiles_dir()
    .join(profile_id)
    .join("last_audit.json")
}

pub fn save_audit_result(result: &AuditResult) -> Result<(), String> {
  let path = audit_path(&result.profile_id);
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let json = serde_json::to_string_pretty(result).map_err(|e| e.to_string())?;
  let tmp = path.with_extension("json.tmp");
  std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
  super::install_registry::replace_file(&tmp, &path)?;
  Ok(())
}

pub fn load_audit_result(profile_id: &str) -> Option<AuditResult> {
  let path = audit_path(profile_id);
  let text = std::fs::read_to_string(path).ok()?;
  serde_json::from_str(&text).ok()
}

async fn cdp_http_client() -> Result<reqwest::Client, String> {
  reqwest::Client::builder()
    .no_proxy()
    .timeout(Duration::from_secs(5))
    .build()
    .map_err(|e| e.to_string())
}

async fn wait_cdp(port: u16) -> Result<(), String> {
  let client = cdp_http_client().await?;
  let url = format!("http://127.0.0.1:{port}/json/version");
  for _ in 0..60 {
    if client.get(&url).send().await.is_ok() {
      return Ok(());
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
  }
  Err(format!("CDP not ready on 127.0.0.1:{port}"))
}

#[derive(Debug, Deserialize)]
struct CdpTarget {
  #[serde(rename = "type")]
  target_type: String,
  #[serde(rename = "webSocketDebuggerUrl")]
  websocket_debugger_url: Option<String>,
}

async fn start_audit_page() -> Result<
  (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
  ),
  String,
> {
  use axum::http::{header, HeaderValue};
  use axum::response::{Html, IntoResponse};
  use axum::routing::get;
  use axum::Router;

  let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .map_err(|e| format!("bind local audit page: {e}"))?;
  let port = listener
    .local_addr()
    .map_err(|e| format!("read local audit page address: {e}"))?
    .port();
  let app = Router::new().route(
    "/",
    get(|| async {
      let mut response = Html(AUDIT_HTML).into_response();
      response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
      );
      response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'none'; script-src 'none'"),
      );
      response
    }),
  );
  let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
  let task = tokio::spawn(async move {
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
      let _ = shutdown_rx.await;
    });
    if let Err(error) = server.await {
      log::warn!("Local audit page server stopped with an error: {error}");
    }
  });
  Ok((format!("http://127.0.0.1:{port}/"), shutdown_tx, task))
}

async fn collect_via_cdp(port: u16, audit_url: &str) -> Result<ObservedFingerprint, String> {
  wait_cdp(port).await?;
  let client = cdp_http_client().await?;
  let targets: Vec<CdpTarget> = client
    .get(format!("http://127.0.0.1:{port}/json"))
    .send()
    .await
    .map_err(|e| e.to_string())?
    .json()
    .await
    .map_err(|e| e.to_string())?;

  let ws_url = targets
    .into_iter()
    .find(|t| t.target_type == "page")
    .and_then(|t| t.websocket_debugger_url)
    .ok_or_else(|| "no page CDP target".to_string())?;

  use futures_util::{SinkExt, StreamExt};
  use tokio_tungstenite::tungstenite::Message;

  let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
    .await
    .map_err(|e| e.to_string())?;

  // Navigate to a trustworthy loopback origin so secure-context-only browser
  // surfaces (WebCrypto, Client Hints, deviceMemory) can be observed.
  let nav = serde_json::json!({
    "id": 1,
    "method": "Page.navigate",
    "params": { "url": audit_url }
  });
  ws.send(Message::Text(nav.to_string().into()))
    .await
    .map_err(|e| e.to_string())?;
  let nav_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
  while tokio::time::Instant::now() < nav_deadline {
    let remaining = nav_deadline.saturating_duration_since(tokio::time::Instant::now());
    let Some(message) = tokio::time::timeout(remaining, ws.next())
      .await
      .map_err(|_| "timeout waiting for audit navigation".to_string())?
    else {
      return Err("CDP connection closed during audit navigation".into());
    };
    let Message::Text(text) = message.map_err(|e| e.to_string())? else {
      continue;
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if value.get("id") == Some(&serde_json::json!(1)) {
      if let Some(error) = value.get("error") {
        return Err(format!("CDP navigation error: {error}"));
      }
      break;
    }
  }

  tokio::time::sleep(Duration::from_millis(500)).await;

  let eval = serde_json::json!({
    "id": 2,
    "method": "Runtime.evaluate",
    "params": {
      "expression": COLLECT_JS,
      "awaitPromise": true,
      "returnByValue": true
    }
  });
  ws.send(Message::Text(eval.to_string().into()))
    .await
    .map_err(|e| e.to_string())?;

  let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
  while tokio::time::Instant::now() < deadline {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(remaining, ws.next()).await {
      let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
      if v.get("id") == Some(&serde_json::json!(2)) {
        if let Some(err) = v.get("error") {
          return Err(format!("CDP evaluate error: {err}"));
        }
        let value = v
          .pointer("/result/result/value")
          .cloned()
          .ok_or_else(|| "missing evaluate result".to_string())?;
        let obs: ObservedFingerprint = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let _ = ws.close(None).await;
        return Ok(obs);
      }
    }
  }
  Err("timeout waiting for CDP evaluate".into())
}

/// Launch a short-lived headed automation session to collect observation.
async fn collect_live(
  profile: &BrowserProfile,
  persona: &FingerprintPersona,
) -> Result<ObservedFingerprint, String> {
  if SessionManager::instance().is_running(profile.id) {
    return Err("profile is already running; stop it before audit launch".into());
  }

  let driver = FingerprintChromiumDriver::new();
  let cdp_port = {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener.local_addr().map_err(|e| e.to_string())?.port()
  };

  // Prefer isolated audit user-data so we don't disturb the real profile lock.
  let audit_dir = crate::app_dirs::cache_dir()
    .join("audit_sessions")
    .join(profile.id.to_string())
    .join(Uuid::new_v4().to_string());
  std::fs::create_dir_all(&audit_dir).map_err(|e| e.to_string())?;

  let upstream = if let Some(ref proxy_id) = profile.proxy_id {
    crate::kernel::geo_consistency::reject_cloud_proxy_id(Some(proxy_id))?;
    Some(
      PROXY_MANAGER
        .get_proxy_settings_by_id(proxy_id)
        .ok_or_else(|| format!("profile proxy not found: {proxy_id}"))?,
    )
  } else {
    None
  };

  let (audit_url, audit_shutdown, audit_task) = start_audit_page().await?;
  let result = async {
    // Always use a short-lived loopback sidecar, including direct profiles, so
    // live audits exercise the same browser routing boundary as real launches.
    let upstream_url = upstream
      .as_ref()
      .map(crate::proxy_manager::ProxyManager::build_proxy_url);
    let worker = crate::proxy_runner::start_proxy_process(upstream_url, None)
      .await
      .map_err(|e| format!("audit proxy worker failed: {e}"))?;
    let worker_id = worker.id.clone();

    let audited = async {
      let port = worker
        .local_port
        .ok_or_else(|| "audit proxy did not report a local port".to_string())?;
      let local_proxy = LocalProxyEndpoint {
        host: "127.0.0.1".into(),
        port,
        protocol: "http".into(),
      };
      let request = KernelLaunchRequest {
        profile: profile.clone(),
        profile_path: audit_dir.clone(),
        url: Some(audit_url.clone()),
        local_proxy: Some(local_proxy.clone()),
        automation: AutomationMode::HeadedAutomation,
        remote_debugging_port: Some(cdp_port),
        headless: false,
        extension_paths: vec![],
        wayfern_config: None,
        persona: Some(persona.clone()),
        proxy_url: Some(local_proxy.proxy_server_arg()),
        ephemeral: true,
      };

      let plan = driver
        .build_launch_plan(&request)
        .map_err(|e| e.to_string())?;
      let mut guard = ProcessGuard::spawn(&plan.executable, &plan.args)?;
      tokio::time::sleep(Duration::from_millis(500)).await;
      if !guard.is_alive() {
        return Err("audit browser exited immediately".into());
      }
      let observation = collect_via_cdp(cdp_port, &audit_url).await;
      let _ = guard.terminate();
      observation
    }
    .await;

    let _ = crate::proxy_runner::stop_proxy_process(&worker_id).await;
    audited
  }
  .await;

  let _ = audit_shutdown.send(());
  let _ = audit_task.await;
  let _ = std::fs::remove_dir_all(&audit_dir);
  result
}

fn load_profile(profile_id: &str) -> Result<BrowserProfile, String> {
  let uuid = Uuid::parse_str(profile_id).map_err(|e| e.to_string())?;
  ProfileManager::instance()
    .list_profiles()
    .map_err(|e| e.to_string())?
    .into_iter()
    .find(|p| p.id == uuid)
    .ok_or_else(|| format!("profile not found: {profile_id}"))
}

/// Static + optional live audit for a profile.
pub async fn run_profile_audit(profile_id: String, live: bool) -> Result<AuditResult, String> {
  let profile = load_profile(&profile_id)?;
  if profile.browser != "fingerprint-chromium" {
    return Err("audit currently supports fingerprint-chromium profiles only".into());
  }
  let persona = ensure_persona(profile.persona.as_ref(), &profile.version)?;

  let (observed, mode) = if live {
    (
      Some(
        collect_live(&profile, &persona)
          .await
          .map_err(|e| format!("live audit collection failed: {e}"))?,
      ),
      "headed_automation",
    )
  } else {
    (None, "static_only")
  };

  let result = evaluate_audit(&profile, &persona, observed, mode);
  save_audit_result(&result)?;
  Ok(result)
}

/// Multiple static/live rounds to verify stability hash (temp sessions when live).
pub async fn run_stability_audit(
  profile_id: String,
  rounds: u32,
  live: bool,
) -> Result<StabilityReport, String> {
  let rounds = rounds.clamp(2, 10);
  let mut hashes = Vec::new();
  let mut findings = Vec::new();
  let mut last = None;

  for _ in 0..rounds {
    let result = run_profile_audit(profile_id.clone(), live).await?;
    hashes.push(result.stability_hash.clone());
    last = Some(result);
  }

  let first = hashes.first().cloned().unwrap_or_default();
  let stable = hashes.iter().all(|h| h == &first);
  if !stable {
    findings.push(finding(
      "STABILITY_HASH_DRIFT",
      AuditStatus::Fail,
      "stability hash changed across audit rounds",
      Some(first),
      Some(hashes.join(",")),
    ));
  }

  Ok(StabilityReport {
    profile_id,
    rounds,
    hashes,
    stable,
    status: if !stable {
      AuditStatus::Fail
    } else {
      last
        .as_ref()
        .map(|result| result.status)
        .unwrap_or(AuditStatus::Unsupported)
    },
    findings,
    last_result: last,
  })
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn run_fingerprint_audit(
  profile_id: String,
  live: Option<bool>,
) -> Result<AuditResult, String> {
  run_profile_audit(profile_id, live.unwrap_or(true)).await
}

#[tauri::command]
pub async fn run_fingerprint_stability_audit(
  profile_id: String,
  rounds: Option<u32>,
  live: Option<bool>,
) -> Result<StabilityReport, String> {
  run_stability_audit(profile_id, rounds.unwrap_or(10), live.unwrap_or(true)).await
}

#[tauri::command]
pub fn get_last_fingerprint_audit(profile_id: String) -> Result<Option<AuditResult>, String> {
  Ok(load_audit_result(&profile_id))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::kernel::persona::{BrowserBrand, FingerprintPlatform, WebRtcPolicy};
  use std::collections::BTreeSet;

  fn sample_persona() -> FingerprintPersona {
    FingerprintPersona {
      schema_version: 1,
      seed: 99,
      platform: FingerprintPlatform::Windows,
      platform_version: Some("15.0.0".into()),
      brand: BrowserBrand::Chrome,
      brand_version: "148".into(),
      language: "en-US".into(),
      accept_languages: vec!["en-US".into()],
      timezone: "America/New_York".into(),
      hardware_concurrency: Some(8),
      window_width: 1920,
      window_height: 1080,
      webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
      spoofing_disabled: BTreeSet::new(),
      proxy_geo_signature: None,
      capability_revision: "t".into(),
    }
  }

  fn sample_profile() -> BrowserProfile {
    BrowserProfile {
      id: Uuid::nil(),
      name: "audit".into(),
      browser: "fingerprint-chromium".into(),
      version: "148.0.7778.215".into(),
      proxy_id: None,
      vpn_id: None,
      launch_hook: None,
      process_id: None,
      last_launch: None,
      release_type: "stable".into(),
      wayfern_config: None,
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
    }
  }

  #[cfg(target_os = "windows")]
  #[tokio::test(flavor = "current_thread")]
  #[ignore = "downloads and launches the audited official fingerprint-chromium kernel"]
  async fn official_kernel_ten_cold_start_audit() {
    let validation_root = tempfile::TempDir::new().unwrap();
    std::env::set_var("COCOBROWSER_DATA_ROOT", validation_root.path());

    let installed = crate::kernel::downloader::install_fingerprint_chromium_148()
      .await
      .expect("official kernel install must pass size, SHA-256, extraction and registry checks");
    assert!(std::path::Path::new(&installed.executable).is_file());

    let mut profile = sample_profile();
    profile.id = Uuid::new_v4();
    profile.name = "official-kernel-live-validation".into();
    profile.persona = Some(
      FingerprintPersona::auto_consistent_windows(&installed.version)
        .expect("audited version must produce a valid Persona"),
    );
    ProfileManager::instance()
      .save_profile(&profile)
      .expect("validation profile must be saved in the isolated data root");

    let report = run_stability_audit(profile.id.to_string(), 10, true)
      .await
      .expect("all live CDP observations must complete");
    assert!(report.stable, "fingerprint hash drifted: {report:#?}");
    assert_ne!(report.status, AuditStatus::Fail, "{report:#?}");
  }

  #[test]
  fn stability_hash_stable_for_same_inputs() {
    let p = sample_persona();
    let h1 = compute_stability_hash(&p, None);
    let h2 = compute_stability_hash(&p, None);
    assert_eq!(h1, h2);
  }

  #[test]
  fn timezone_mismatch_fails() {
    let profile = sample_profile();
    let persona = sample_persona();
    let obs = ObservedFingerprint {
      timezone: Some("Asia/Tokyo".into()),
      webdriver: Some(false),
      user_agent: Some("Mozilla/5.0 Chrome/148.0.0.0".into()),
      ..Default::default()
    };
    let r = evaluate_audit(&profile, &persona, Some(obs), "test");
    assert_eq!(r.status, AuditStatus::Fail);
    assert!(r
      .consistency_errors
      .iter()
      .any(|f| f.code == "TIMEZONE_MISMATCH"));
  }

  #[test]
  fn webrtc_host_candidate_is_leak() {
    let profile = sample_profile();
    let persona = sample_persona();
    let obs = ObservedFingerprint {
      timezone: Some(persona.timezone.clone()),
      webdriver: Some(false),
      user_agent: Some("Chrome/148".into()),
      webrtc_candidates: Some(vec![
        "candidate:1 1 UDP 2122 192.168.1.5 54321 typ host".into()
      ]),
      ..Default::default()
    };
    let r = evaluate_audit(&profile, &persona, Some(obs), "test");
    assert!(r
      .leak_findings
      .iter()
      .any(|f| f.code == "WEBRTC_HOST_CANDIDATE"));
    assert_eq!(r.status, AuditStatus::Fail);
  }

  #[test]
  fn mdns_webrtc_host_candidate_is_not_an_ip_leak() {
    assert_eq!(
      exposed_webrtc_host_address("candidate:1 1 UDP 2122 e7f.local 54321 typ host generation 0"),
      None
    );
  }

  #[test]
  fn matching_observation_passes_or_warns_only() {
    let profile = sample_profile();
    let persona = sample_persona();
    let obs = ObservedFingerprint {
      timezone: Some("America/New_York".into()),
      language: Some("en-US".into()),
      webdriver: Some(false),
      user_agent: Some("Mozilla/5.0 Chrome/148.0.7778.215".into()),
      hardware_concurrency: Some(8),
      canvas_hash: Some("canvas".into()),
      audio_hash: Some("audio".into()),
      webgl_vendor: Some("Intel Inc.".into()),
      webgl_renderer: Some("Intel Iris Xe".into()),
      ..Default::default()
    };
    let r = evaluate_audit(&profile, &persona, Some(obs), "test");
    assert_ne!(r.status, AuditStatus::Fail);
  }
}
