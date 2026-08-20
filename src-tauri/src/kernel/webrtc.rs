use super::persona::WebRtcPolicy;
use std::path::PathBuf;

const MANIFEST_FILE: &str = "manifest.json";
const BLOCKER_FILE: &str = "block-webrtc.js";
const PASSTHROUGH_JS: &str = "(() => {})();\n";

const BLOCKER_JS: &str = r#"(() => {
  const names = [
    "RTCPeerConnection",
    "webkitRTCPeerConnection",
    "RTCSessionDescription",
    "RTCIceCandidate",
    "RTCDataChannel",
    "RTCDtlsTransport",
    "RTCIceTransport",
    "RTCSctpTransport",
    "RTCRtpSender",
    "RTCRtpReceiver",
    "RTCRtpTransceiver"
  ];
  for (const name of names) {
    try {
      delete globalThis[name];
    } catch (_) {}
    if (globalThis[name] !== undefined) {
      try {
        Object.defineProperty(globalThis, name, {
          value: undefined,
          configurable: false,
          writable: false
        });
      } catch (_) {}
    }
  }
})();
"#;

fn manifest_json() -> String {
  serde_json::json!({
    "manifest_version": 3,
    "name": "WebRTC",
    "version": "1.0",
    "content_scripts": [{
      "matches": ["<all_urls>"],
      "js": [BLOCKER_FILE],
      "run_at": "document_start",
      "world": "MAIN",
      "all_frames": true,
      "match_about_blank": true,
      "match_origin_as_fallback": true
    }]
  })
  .to_string()
}

fn extension_dir(profile_id: &str) -> PathBuf {
  crate::profile::ProfileManager::instance()
    .get_profiles_dir()
    .join(profile_id)
    .join("webrtc-blocker-ext")
}

fn script_for_policy(policy: WebRtcPolicy) -> &'static str {
  if policy == WebRtcPolicy::Disabled {
    BLOCKER_JS
  } else {
    PASSTHROUGH_JS
  }
}

pub(crate) fn extension_for_policy(
  profile_id: &str,
  policy: WebRtcPolicy,
) -> Result<String, String> {
  let dir = extension_dir(profile_id);
  std::fs::create_dir_all(&dir)
    .map_err(|error| format!("failed to create WebRTC control extension: {error}"))?;
  std::fs::write(dir.join(MANIFEST_FILE), manifest_json())
    .map_err(|error| format!("failed to write WebRTC control manifest: {error}"))?;
  // Always rewrite the script. Chromium can remember command-line-loaded
  // unpacked extensions in a persistent profile, so a no-op script is what
  // reliably clears a previous Disabled launch.
  std::fs::write(dir.join(BLOCKER_FILE), script_for_policy(policy))
    .map_err(|error| format!("failed to write WebRTC control script: {error}"))?;

  Ok(dir.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_disabled_mode_gets_the_blocker_script() {
    for policy in [
      WebRtcPolicy::Replace,
      WebRtcPolicy::Privacy,
      WebRtcPolicy::Allow,
    ] {
      assert_eq!(script_for_policy(policy), PASSTHROUGH_JS);
    }
    assert_eq!(script_for_policy(WebRtcPolicy::Disabled), BLOCKER_JS);
  }

  #[test]
  fn blocker_runs_in_the_page_world_before_page_scripts() {
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
    let script = &manifest["content_scripts"][0];
    assert_eq!(script["run_at"], "document_start");
    assert_eq!(script["world"], "MAIN");
    assert_eq!(script["all_frames"], true);
    assert!(BLOCKER_JS.contains("RTCPeerConnection"));
  }
}
