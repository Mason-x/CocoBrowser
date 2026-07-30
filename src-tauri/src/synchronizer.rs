use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::sync::Mutex as AsyncMutex;

use crate::profile::manager::ProfileManager;
use crate::profile::types::BrowserProfile;

/// Maximum number of profiles to launch concurrently
const MAX_CONCURRENT_LAUNCHES: usize = 5;

/// Event captured from the leader browser via Wayfern.inputCaptured CDP events.
/// Fields match the Wayfern.inputCaptured event schema directly.
/// The only kernel this fork ships.
const SUPPORTED_BROWSER: &str = "fingerprint-chromium";

/// Name of the isolated world the capture script runs in.
const CAPTURE_WORLD: &str = "coco_browse_together";

/// Name of the CDP binding the capture script calls.
const CAPTURE_BINDING: &str = "__cocoBrowseTogether";

/// Input capture for the leader page.
///
/// The upstream engine exposed a proprietary `Wayfern.enableInputCapture` CDP
/// command that captured input in the browser process. Standard CDP has no
/// equivalent — `Input.dispatch*` sends input, it cannot observe it — so this
/// fork listens in the page instead.
///
/// The script is installed into a named isolated world, so the page's own
/// JavaScript cannot enumerate these listeners or reach the binding: that keeps
/// the observable surface of a synchronized profile the same as an unsynchronized
/// one, which matters because this is a fingerprinting-sensitive browser.
/// Listeners run in the capture phase so a page calling `stopPropagation` on its
/// own handlers cannot hide input from us.
const CAPTURE_SCRIPT: &str = r#"
(() => {
  const send = (payload) => {
    try {
      globalThis.__cocoBrowseTogether(JSON.stringify(payload));
    } catch (_) {
      /* binding not attached yet; drop the event rather than throw into the page */
    }
  };

  // CDP modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8
  const mods = (e) =>
    (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);

  const BUTTONS = ["left", "middle", "right"];

  const onMouse = (type) => (e) => {
    send({
      type,
      x: e.clientX,
      y: e.clientY,
      button: BUTTONS[e.button] || "left",
      clickCount: e.detail || 1,
      modifiers: mods(e),
      timestamp: Date.now(),
    });
  };

  addEventListener("mousedown", onMouse("mousedown"), true);
  addEventListener("mouseup", onMouse("mouseup"), true);

  addEventListener(
    "wheel",
    (e) => {
      send({ type: "wheel", deltaX: e.deltaX, deltaY: e.deltaY, timestamp: Date.now() });
    },
    true,
  );

  const keyPayload = (type, e) => ({
    type,
    key: e.key,
    code: e.code,
    windowsVirtualKeyCode: e.keyCode,
    modifiers: mods(e),
    timestamp: Date.now(),
  });

  addEventListener(
    "keydown",
    (e) => {
      send(keyPayload("keydown", e));
      // A printable keypress also needs a `char` event or the follower receives
      // the keystroke without any text being inserted.
      if (e.key && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
        send({ type: "char", text: e.key, modifiers: mods(e), timestamp: Date.now() });
      }
    },
    true,
  );

  addEventListener("keyup", (e) => send(keyPayload("keyup", e)), true);
})();
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedEvent {
  #[serde(rename = "type")]
  pub event_type: String,
  #[serde(default)]
  pub url: Option<String>,
  #[serde(default)]
  pub x: Option<f64>,
  #[serde(default)]
  pub y: Option<f64>,
  #[serde(default)]
  pub button: Option<String>,
  #[serde(default, rename = "clickCount")]
  pub click_count: Option<i32>,
  #[serde(default)]
  pub key: Option<String>,
  #[serde(default)]
  pub code: Option<String>,
  #[serde(default, rename = "windowsVirtualKeyCode")]
  pub key_code: Option<i32>,
  #[serde(default)]
  pub modifiers: Option<i32>,
  #[serde(default)]
  pub text: Option<String>,
  #[serde(default, rename = "deltaX")]
  pub delta_x: Option<f64>,
  #[serde(default, rename = "deltaY")]
  pub delta_y: Option<f64>,
  #[serde(default)]
  pub timestamp: Option<f64>,
}

// No JavaScript injection needed — Wayfern.enableInputCapture handles everything natively.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFollowerState {
  pub profile_id: String,
  pub profile_name: String,
  /// None = healthy, Some(url) = desynced at this URL
  pub failed_at_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSessionInfo {
  pub id: String,
  pub leader_profile_id: String,
  pub leader_profile_name: String,
  pub followers: Vec<SyncFollowerState>,
}

/// Internal session state
struct SyncSession {
  id: String,
  leader_profile_id: String,
  leader_profile_name: String,
  followers: HashMap<String, SyncFollowerState>,
  /// Cancellation token — drop sender to stop the listener task
  cancel_tx: tokio::sync::watch::Sender<bool>,
  /// Loopback CDP port per profile id, assigned when this session launched the
  /// browsers. Browse-together is the only reason these profiles run with
  /// remote debugging enabled, and the ports live only as long as the session.
  cdp_ports: HashMap<String, u16>,
}

pub struct SynchronizerManager {
  inner: Arc<AsyncMutex<SynchronizerInner>>,
}

struct SynchronizerInner {
  sessions: HashMap<String, SyncSession>,
}

static SYNCHRONIZER: std::sync::OnceLock<SynchronizerManager> = std::sync::OnceLock::new();

impl SynchronizerManager {
  pub fn instance() -> &'static SynchronizerManager {
    SYNCHRONIZER.get_or_init(|| SynchronizerManager {
      inner: Arc::new(AsyncMutex::new(SynchronizerInner {
        sessions: HashMap::new(),
      })),
    })
  }

  /// Start a new sync session. Launches all profiles and begins event capture.
  pub async fn start_session(
    &self,
    app_handle: tauri::AppHandle,
    leader_profile_id: String,
    follower_profile_ids: Vec<String>,
  ) -> Result<SyncSessionInfo, String> {
    let profiles = ProfileManager::instance()
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles: {e}"))?;

    let leader = profiles
      .iter()
      .find(|p| p.id.to_string() == leader_profile_id)
      .ok_or("Leader profile not found")?
      .clone();

    if leader.browser != SUPPORTED_BROWSER {
      return Err(format!(
        "Browse-together requires a {SUPPORTED_BROWSER} profile."
      ));
    }

    // Check leader is not already running
    if leader.process_id.is_some() {
      let sys = sysinfo::System::new_all();
      if let Some(pid) = leader.process_id {
        if sys.process(sysinfo::Pid::from(pid as usize)).is_some() {
          return Err(
            "Leader profile is already running. Stop it first to start a sync session.".to_string(),
          );
        }
      }
    }

    let mut follower_profiles: Vec<BrowserProfile> = Vec::new();
    for fid in &follower_profile_ids {
      let fp = profiles
        .iter()
        .find(|p| p.id.to_string() == *fid)
        .ok_or(format!("Follower profile '{fid}' not found"))?
        .clone();
      if fp.browser != SUPPORTED_BROWSER {
        return Err(format!(
          "Profile '{}' cannot be synchronized: browse-together requires a {SUPPORTED_BROWSER} profile.",
          fp.name
        ));
      }
      follower_profiles.push(fp);
    }

    // Check no profile is part of another active session
    {
      let inner = self.inner.lock().await;
      for session in inner.sessions.values() {
        if session.leader_profile_id == leader_profile_id {
          return Err("Leader profile is already in another sync session.".to_string());
        }
        for fid in &follower_profile_ids {
          if session.leader_profile_id == *fid || session.followers.contains_key(fid) {
            return Err(format!(
              "Profile '{fid}' is already part of another sync session."
            ));
          }
        }
      }
    }

    let session_id = uuid::Uuid::new_v4().to_string();

    log::info!(
      "Synchronizer: launching leader '{}' and {} followers",
      leader.name,
      follower_profiles.len()
    );

    // Reserve one loopback CDP port per profile. Browse-together is the only
    // path that launches with remote debugging: a normal manual launch stays
    // debugger-free, and these ports disappear with the session.
    let mut cdp_ports: HashMap<String, u16> = HashMap::new();
    let leader_port = Self::reserve_loopback_port()?;
    cdp_ports.insert(leader_profile_id.clone(), leader_port);
    for fp in &follower_profiles {
      cdp_ports.insert(fp.id.to_string(), Self::reserve_loopback_port()?);
    }

    // Launch leader first so it gets focus
    crate::browser_runner::BrowserRunner::instance()
      .launch_browser_with_debugging(app_handle.clone(), &leader, None, Some(leader_port), false)
      .await
      .map_err(|e| format!("Failed to launch leader: {e}"))?;

    // Launch followers in parallel batches of MAX_CONCURRENT_LAUNCHES
    for chunk in follower_profiles.chunks(MAX_CONCURRENT_LAUNCHES) {
      let mut set = tokio::task::JoinSet::new();
      for fp in chunk {
        let ah = app_handle.clone();
        let fp = fp.clone();
        let port = cdp_ports[&fp.id.to_string()];
        set.spawn(async move {
          crate::browser_runner::BrowserRunner::instance()
            .launch_browser_with_debugging(ah, &fp, None, Some(port), false)
            .await
            .map_err(|e| (fp.name.clone(), e.to_string()))
        });
      }
      while let Some(result) = set.join_next().await {
        match result {
          Ok(Ok(_)) => {}
          Ok(Err((name, e))) => {
            log::error!("Failed to launch follower '{name}': {e}");
            // Kill leader and all already-launched followers
            let _ =
              crate::browser_runner::kill_browser_profile(app_handle.clone(), leader.clone()).await;
            for fp in &follower_profiles {
              let _ =
                crate::browser_runner::kill_browser_profile(app_handle.clone(), fp.clone()).await;
            }
            return Err(format!("Failed to launch follower '{name}': {e}"));
          }
          Err(e) => {
            log::error!("Launch task panicked: {e}");
            let _ =
              crate::browser_runner::kill_browser_profile(app_handle.clone(), leader.clone()).await;
            return Err(format!("Launch task panicked: {e}"));
          }
        }
      }
    }

    // Bring leader window to front after all followers launched
    Self::focus_leader_window(&leader).await;

    // Build follower states
    let mut followers = HashMap::new();
    for fp in &follower_profiles {
      followers.insert(
        fp.id.to_string(),
        SyncFollowerState {
          profile_id: fp.id.to_string(),
          profile_name: fp.name.clone(),
          failed_at_url: None,
        },
      );
    }

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

    let session = SyncSession {
      id: session_id.clone(),
      leader_profile_id: leader_profile_id.clone(),
      leader_profile_name: leader.name.clone(),
      followers: followers.clone(),
      cancel_tx,
      cdp_ports: cdp_ports.clone(),
    };

    let info = SyncSessionInfo {
      id: session_id.clone(),
      leader_profile_id: leader_profile_id.clone(),
      leader_profile_name: leader.name.clone(),
      followers: followers.values().cloned().collect(),
    };

    {
      let mut inner = self.inner.lock().await;
      inner.sessions.insert(session_id.clone(), session);
    }

    // Emit initial session event
    let _ = app_handle.emit("sync-session-changed", &info);

    // Spawn the CDP listener task with a readiness signal
    let manager = self.inner.clone();
    let ah = app_handle.clone();
    let sid = session_id.clone();
    let lid = leader_profile_id.clone();
    let fids: Vec<String> = follower_profile_ids.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let all_profile_ids: Vec<String> = std::iter::once(leader_profile_id.clone())
      .chain(follower_profile_ids.iter().cloned())
      .collect();

    log::info!("Synchronizer: spawning CDP listener task");

    tokio::spawn(async move {
      log::info!("Synchronizer: CDP listener task started");
      if let Err(e) = Self::run_session_loop(
        ah.clone(),
        manager.clone(),
        sid.clone(),
        lid,
        fids,
        cancel_rx,
        ready_tx,
      )
      .await
      {
        log::error!("Synchronizer session {sid} error: {e}");
        // Kill all profiles on error (leader + followers)
        for pid in &all_profile_ids {
          if let Ok(p) = Self::get_profile(pid) {
            let _ = crate::browser_runner::kill_browser_profile(ah.clone(), p).await;
          }
        }
      }
      // Session ended — clean up
      let mut inner = manager.lock().await;
      inner.sessions.remove(&sid);
      let _ = ah.emit("sync-session-ended", &sid);
    });

    // Wait for the CDP session to be ready (or fail)
    match tokio::time::timeout(std::time::Duration::from_secs(90), ready_rx).await {
      Ok(Ok(Ok(()))) => Ok(info),
      Ok(Ok(Err(e))) => Err(format!("Synchronizer setup failed: {e}")),
      Ok(Err(_)) => Err("Synchronizer setup channel closed unexpectedly".to_string()),
      Err(_) => Err("Synchronizer setup timed out".to_string()),
    }
  }

  /// Bring the leader browser window to front.
  ///
  /// On macOS this is a no-op on purpose: the only way to raise another
  /// app's window from Rust is via `osascript` / Apple Events, which
  /// triggers the TCC "prevented from modifying other apps" prompt. Coco
  /// must never touch other apps on the user's Mac.
  async fn focus_leader_window(leader: &BrowserProfile) {
    let profile = match Self::get_profile(&leader.id.to_string()) {
      Ok(p) => p,
      Err(_) => return,
    };
    let Some(pid) = profile.process_id else {
      return;
    };

    #[cfg(target_os = "linux")]
    {
      let _ = tokio::process::Command::new("xdotool")
        .args([
          "search",
          "--pid",
          &pid.to_string(),
          "--onlyvisible",
          "windowactivate",
        ])
        .output()
        .await;
    }

    #[cfg(not(target_os = "linux"))]
    {
      let _ = pid;
    }
  }

  /// Core session loop: inject capture script on leader, listen for events, replay on followers.
  async fn run_session_loop(
    app_handle: tauri::AppHandle,
    manager: Arc<AsyncMutex<SynchronizerInner>>,
    session_id: String,
    leader_profile_id: String,
    follower_profile_ids: Vec<String>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    ready_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
  ) -> Result<(), String> {
    use futures_util::sink::SinkExt;
    use futures_util::stream::StreamExt;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    log::info!("Synchronizer: run_session_loop started, waiting 1s for browsers");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Connect to leader page-level target for reliable event capture
    log::info!("Synchronizer: getting leader CDP port");
    let leader_port = Self::get_cdp_port(&manager, &session_id, &leader_profile_id).await?;
    log::info!("Synchronizer: leader CDP port = {leader_port}, getting WS URL");
    let leader_ws_url = Self::get_page_ws_url(leader_port).await?;

    log::info!("Synchronizer: connecting to leader page at {leader_ws_url}");

    let (mut ws_stream, _) = connect_async(&leader_ws_url)
      .await
      .map_err(|e| format!("Failed to connect to leader CDP: {e}"))?;

    // Helper: send command and collect response, buffering non-response events
    let mut cmd_id: u64 = 0;
    let mut pending_events: Vec<serde_json::Value> = Vec::new();

    // Send a CDP command and wait for its response, buffering events that arrive in between
    async fn send_cmd(
      ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
      >,
      cmd_id: &mut u64,
      pending_events: &mut Vec<serde_json::Value>,
      method: &str,
      params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
      *cmd_id += 1;
      let id = *cmd_id;
      let cmd = serde_json::json!({ "id": id, "method": method, "params": params });
      ws.send(Message::Text(cmd.to_string().into()))
        .await
        .map_err(|e| format!("Failed to send {method}: {e}"))?;
      let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
      loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
          return Err(format!("Timeout waiting for {method} response"));
        }
        match tokio::time::timeout(remaining, ws.next()).await {
          Ok(Some(Ok(Message::Text(text)))) => {
            let resp: serde_json::Value = serde_json::from_str(text.as_str()).unwrap_or_default();
            if resp.get("id") == Some(&serde_json::json!(id)) {
              if let Some(error) = resp.get("error") {
                return Err(format!("CDP error for {method}: {error}"));
              }
              return Ok(resp.get("result").cloned().unwrap_or(serde_json::json!({})));
            }
            // Buffer events that arrive while waiting for response
            if resp.get("method").is_some() {
              pending_events.push(resp);
            }
          }
          Ok(Some(Ok(_))) => continue,
          Ok(Some(Err(e))) => return Err(format!("WebSocket error: {e}")),
          Ok(None) => return Err("WebSocket closed".to_string()),
          Err(_) => return Err(format!("Timeout waiting for {method} response")),
        }
      }
    }

    // Install the capture script into a named isolated world and expose a
    // binding it can call. `Runtime.addBinding` is scoped to the same world, so
    // the page's own JavaScript can neither see the function nor enumerate the
    // listeners. `runImmediately` covers the already-loaded document in addition
    // to future navigations.
    let setup_commands: Vec<(&str, serde_json::Value)> = vec![
      ("Page.enable", serde_json::json!({})),
      ("Runtime.enable", serde_json::json!({})),
      (
        "Runtime.addBinding",
        serde_json::json!({
          "name": CAPTURE_BINDING,
          "executionContextName": CAPTURE_WORLD,
        }),
      ),
      (
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({
          "source": CAPTURE_SCRIPT,
          "worldName": CAPTURE_WORLD,
          "runImmediately": true,
        }),
      ),
    ];

    for (method, params) in setup_commands {
      match send_cmd(
        &mut ws_stream,
        &mut cmd_id,
        &mut pending_events,
        method,
        params,
      )
      .await
      {
        Ok(_) => log::info!("Synchronizer: {method} OK"),
        Err(e) => {
          log::error!("Synchronizer: {method} FAILED: {e}");
          return Err(format!("{method} failed: {e}"));
        }
      }
    }

    log::info!("Synchronizer: input capture enabled");

    // Get leader window size and resize all followers to match
    let leader_bounds = send_cmd(
      &mut ws_stream,
      &mut cmd_id,
      &mut pending_events,
      "Browser.getWindowForTarget",
      serde_json::json!({}),
    )
    .await
    .ok();

    if let Some(bounds_result) = &leader_bounds {
      if let Some(bounds) = bounds_result.get("bounds") {
        let width = bounds.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
        let height = bounds.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
        if width > 0 && height > 0 {
          log::info!("Synchronizer: leader window size {width}x{height}, resizing followers");
          for fid in &follower_profile_ids {
            {
              if let Ok(port) = Self::get_cdp_port(&manager, &session_id, fid).await {
                if let Ok(f_ws) = Self::get_page_ws_url(port).await {
                  if let Ok((mut fws, _)) = tokio_tungstenite::connect_async(&f_ws).await {
                    // Get follower's window ID
                    let get_win = serde_json::json!({ "id": 1, "method": "Browser.getWindowForTarget", "params": {} });
                    let _ = fws.send(Message::Text(get_win.to_string().into())).await;
                    if let Some(Ok(Message::Text(text))) = fws.next().await {
                      if let Ok(resp) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                        if let Some(win_id) = resp
                          .get("result")
                          .and_then(|r| r.get("windowId"))
                          .and_then(|w| w.as_i64())
                        {
                          let set_bounds = serde_json::json!({
                            "id": 2,
                            "method": "Browser.setWindowBounds",
                            "params": { "windowId": win_id, "bounds": { "width": width, "height": height } }
                          });
                          let _ = fws.send(Message::Text(set_bounds.to_string().into())).await;
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }

    log::info!("Synchronizer: opening persistent connections to followers");

    // Open persistent WebSocket connections to each follower and create event channels.
    // Each follower gets a dedicated replay task with a long-lived WS connection.
    let mut follower_senders: HashMap<String, tokio::sync::mpsc::UnboundedSender<CapturedEvent>> =
      HashMap::new();

    for fid in &follower_profile_ids {
      match Self::get_profile(fid) {
        Ok(fp) => match Self::get_cdp_port(&manager, &session_id, fid).await {
          Ok(port) => match Self::get_page_ws_url(port).await {
            Ok(url) => {
              match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => {
                  log::info!("Synchronizer: follower {} connected at {}", fp.name, url);
                  let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<CapturedEvent>();
                  follower_senders.insert(fid.clone(), tx);

                  // Spawn dedicated replay task for this follower
                  let fid_clone = fid.clone();
                  let mgr = manager.clone();
                  let sid = session_id.clone();
                  let ah = app_handle.clone();
                  tokio::spawn(async move {
                    Self::follower_replay_loop(ws, rx, fid_clone, mgr, sid, ah).await;
                  });
                }
                Err(e) => log::warn!(
                  "Synchronizer: failed to connect to follower {}: {e}",
                  fp.name
                ),
              }
            }
            Err(e) => log::warn!(
              "Synchronizer: failed to get WS URL for follower {}: {e}",
              fp.name
            ),
          },
          Err(e) => log::warn!(
            "Synchronizer: failed to get CDP port for follower {}: {e}",
            fp.name
          ),
        },
        Err(e) => log::warn!("Synchronizer: failed to get follower profile {fid}: {e}"),
      }
    }

    log::info!(
      "Synchronizer: {} of {} followers connected",
      follower_senders.len(),
      follower_profile_ids.len()
    );

    // Track when the last user interaction was captured (for suppressing click-caused nav replay)
    let mut last_user_event_time = std::time::Instant::now() - std::time::Duration::from_secs(60);

    // Signal that the session is ready for interaction
    let _ = ready_tx.send(Ok(()));

    // Process any events that were buffered during setup
    for event in pending_events.drain(..) {
      Self::handle_cdp_event(
        &event,
        &app_handle,
        &manager,
        &session_id,
        &follower_senders,
        false,
      )
      .await;
    }

    // Main event loop — listen for Wayfern.inputCaptured events
    loop {
      tokio::select! {
          _ = cancel_rx.changed() => {
              if *cancel_rx.borrow() {
                  log::info!("Synchronizer session {session_id}: cancelled");
                  break;
              }
          }
          msg = ws_stream.next() => {
              match msg {
                  Some(Ok(Message::Text(text))) => {
                      let value: serde_json::Value = match serde_json::from_str(text.as_str()) {
                          Ok(v) => v,
                          Err(_) => continue,
                      };


                      // Log CDP command response errors
                      if let Some(id) = value.get("id") {
                          if let Some(error) = value.get("error") {
                              log::warn!("Synchronizer: CDP command {id} error: {error}");
                          }
                      }

                      // Track user interaction timing. A navigation that follows
                      // real input within the window is the *result* of that input,
                      // so the follower must not also be told to navigate.
                      if Self::is_capture_binding_call(&value) {
                          last_user_event_time = std::time::Instant::now();
                      }

                      let recent_user_event = last_user_event_time.elapsed() < std::time::Duration::from_secs(2);

                      Self::handle_cdp_event(
                          &value,
                          &app_handle,
                          &manager,
                          &session_id,
                          &follower_senders,
                          recent_user_event,
                      ).await;
                  }
                  Some(Ok(_)) => {} // pings, binary, etc.
                  Some(Err(e)) => {
                      log::error!("Synchronizer: leader WebSocket error: {e}");
                      break;
                  }
                  None => {
                      log::info!("Synchronizer: leader WebSocket closed (browser closed)");
                      break;
                  }
              }
          }
      }
    }

    // Leader closed or session cancelled — kill all followers
    log::info!("Synchronizer session {session_id}: stopping all followers");
    let follower_ids: Vec<String> = {
      let inner = manager.lock().await;
      if let Some(session) = inner.sessions.get(&session_id) {
        session.followers.keys().cloned().collect()
      } else {
        Vec::new()
      }
    };

    for fid in follower_ids {
      if let Ok(fp) = Self::get_profile(&fid) {
        let _ = crate::browser_runner::kill_browser_profile(app_handle.clone(), fp).await;
      }
    }

    Ok(())
  }

  /// True when this CDP message is a capture-binding call from our isolated world.
  fn is_capture_binding_call(value: &serde_json::Value) -> bool {
    value.get("method").and_then(|m| m.as_str()) == Some("Runtime.bindingCalled")
      && value
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        == Some(CAPTURE_BINDING)
  }

  /// Handle a single CDP event from the leader
  async fn handle_cdp_event(
    value: &serde_json::Value,
    _app_handle: &tauri::AppHandle,
    _manager: &Arc<AsyncMutex<SynchronizerInner>>,
    _session_id: &str,
    follower_senders: &HashMap<String, tokio::sync::mpsc::UnboundedSender<CapturedEvent>>,
    recent_user_event: bool,
  ) {
    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");

    // Input arrives as a binding call from the isolated-world capture script.
    // The payload is the JSON the script produced, matching `CapturedEvent`.
    if Self::is_capture_binding_call(value) {
      {
        if let Some(payload) = value
          .get("params")
          .and_then(|p| p.get("payload"))
          .and_then(|v| v.as_str())
        {
          match serde_json::from_str::<CapturedEvent>(payload) {
            Ok(event) => {
              // mousemove is never emitted by the script, but stay defensive:
              // replaying it would flood the followers for no benefit.
              if event.event_type == "mousemove" {
                return;
              }
              log::debug!("Synchronizer: captured {}", event.event_type);
              for tx in follower_senders.values() {
                let _ = tx.send(event.clone());
              }
            }
            Err(e) => {
              log::warn!("Synchronizer: unparsable capture payload: {e}");
            }
          }
        }
      }
    }

    // Handle Page.frameNavigated — replay only for address-bar navigations
    if method == "Page.frameNavigated" && !recent_user_event {
      if let Some(params) = value.get("params") {
        if let Some(frame) = params.get("frame") {
          let is_top = frame.get("parentId").is_none();
          if is_top {
            if let Some(url) = frame.get("url").and_then(|v| v.as_str()) {
              if !url.starts_with("about:") && !url.starts_with("chrome://") {
                log::info!("Synchronizer: replaying address-bar navigation to {url}");
                let nav_event = CapturedEvent {
                  event_type: "navigate".to_string(),
                  url: Some(url.to_string()),
                  x: None,
                  y: None,
                  button: None,
                  click_count: None,
                  key: None,
                  code: None,
                  key_code: None,
                  modifiers: None,
                  text: None,
                  delta_x: None,
                  delta_y: None,
                  timestamp: None,
                };
                for tx in follower_senders.values() {
                  let _ = tx.send(nav_event.clone());
                }
              }
            }
          }
        }
      }
    }
  }

  /// Dedicated replay loop for a single follower with a persistent WebSocket connection.
  /// Processes events from the channel sequentially — no per-event connection overhead.
  async fn follower_replay_loop(
    mut ws: tokio_tungstenite::WebSocketStream<
      tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<CapturedEvent>,
    follower_id: String,
    manager: Arc<AsyncMutex<SynchronizerInner>>,
    session_id: String,
    app_handle: tauri::AppHandle,
  ) {
    use futures_util::sink::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let mut cmd_id: u64 = 0;

    while let Some(event) = rx.recv().await {
      cmd_id += 1;
      let button = event.button.clone().unwrap_or_else(|| "left".to_string());

      let command = match event.event_type.as_str() {
        "navigate" => event
          .url
          .as_ref()
          .map(|url| ("Page.navigate", serde_json::json!({ "url": url }))),
        "mousedown" => Some((
          "Input.dispatchMouseEvent",
          serde_json::json!({
            "type": "mousePressed",
            "x": event.x.unwrap_or(0.0),
            "y": event.y.unwrap_or(0.0),
            "button": button,
            "clickCount": event.click_count.unwrap_or(1),
            "modifiers": event.modifiers.unwrap_or(0),
          }),
        )),
        "mouseup" => Some((
          "Input.dispatchMouseEvent",
          serde_json::json!({
            "type": "mouseReleased",
            "x": event.x.unwrap_or(0.0),
            "y": event.y.unwrap_or(0.0),
            "button": button,
            "modifiers": event.modifiers.unwrap_or(0),
          }),
        )),
        "keydown" => Some((
          "Input.dispatchKeyEvent",
          serde_json::json!({
            "type": "keyDown",
            "key": event.key.clone().unwrap_or_default(),
            "code": event.code.clone().unwrap_or_default(),
            "windowsVirtualKeyCode": event.key_code.unwrap_or(0),
            "modifiers": event.modifiers.unwrap_or(0),
          }),
        )),
        "keyup" => Some((
          "Input.dispatchKeyEvent",
          serde_json::json!({
            "type": "keyUp",
            "key": event.key.clone().unwrap_or_default(),
            "code": event.code.clone().unwrap_or_default(),
            "windowsVirtualKeyCode": event.key_code.unwrap_or(0),
            "modifiers": event.modifiers.unwrap_or(0),
          }),
        )),
        "char" => {
          let text = event.text.clone().unwrap_or_default();
          if text.is_empty() {
            None
          } else {
            Some((
              "Input.dispatchKeyEvent",
              serde_json::json!({
                "type": "char",
                "text": text,
                "unmodifiedText": text,
                "modifiers": event.modifiers.unwrap_or(0),
              }),
            ))
          }
        }
        "wheel" => {
          let dx = -event.delta_x.unwrap_or(0.0);
          let dy = -event.delta_y.unwrap_or(0.0);
          Some((
            "Runtime.evaluate",
            serde_json::json!({
              "expression": format!("window.scrollBy({dx},{dy})"),
            }),
          ))
        }
        _ => None,
      };

      if let Some((method, params)) = command {
        let cmd = serde_json::json!({ "id": cmd_id, "method": method, "params": params });
        if let Err(e) = ws.send(Message::Text(cmd.to_string().into())).await {
          log::warn!("Synchronizer: follower {follower_id} send failed: {e}");
          // Mark as desynced
          let mut inner = manager.lock().await;
          if let Some(session) = inner.sessions.get_mut(&session_id) {
            if let Some(follower) = session.followers.get_mut(&follower_id) {
              follower.failed_at_url = Some("connection lost".to_string());
              let info = SyncSessionInfo {
                id: session.id.clone(),
                leader_profile_id: session.leader_profile_id.clone(),
                leader_profile_name: session.leader_profile_name.clone(),
                followers: session.followers.values().cloned().collect(),
              };
              let _ = app_handle.emit("sync-session-changed", &info);
            }
          }
          break;
        }
        // Don't wait for response — fire and forget for speed.
        // CDP commands are processed in order by Chromium.
      }
    }
  }

  /// Stop a sync session by ID. Kills all followers.
  pub async fn stop_session(
    &self,
    app_handle: tauri::AppHandle,
    session_id: &str,
  ) -> Result<(), String> {
    let mut inner = self.inner.lock().await;
    let session = inner
      .sessions
      .remove(session_id)
      .ok_or("Session not found")?;

    // Signal the listener task to stop
    let _ = session.cancel_tx.send(true);

    // Kill followers
    for fid in session.followers.keys() {
      if let Ok(fp) = Self::get_profile(fid) {
        let _ = crate::browser_runner::kill_browser_profile(app_handle.clone(), fp).await;
      }
    }

    // Kill leader
    if let Ok(leader) = Self::get_profile(&session.leader_profile_id) {
      let _ = crate::browser_runner::kill_browser_profile(app_handle.clone(), leader).await;
    }

    let _ = app_handle.emit("sync-session-ended", session_id);
    Ok(())
  }

  /// Remove a single follower from an active session (user clicked stop on follower).
  pub async fn remove_follower(
    &self,
    app_handle: tauri::AppHandle,
    session_id: &str,
    follower_profile_id: &str,
  ) -> Result<(), String> {
    let mut inner = self.inner.lock().await;
    let session = inner
      .sessions
      .get_mut(session_id)
      .ok_or("Session not found")?;

    session.followers.remove(follower_profile_id);

    // Kill the follower browser
    if let Ok(fp) = Self::get_profile(follower_profile_id) {
      let _ = crate::browser_runner::kill_browser_profile(app_handle.clone(), fp).await;
    }

    // Emit updated session info
    let info = SyncSessionInfo {
      id: session.id.clone(),
      leader_profile_id: session.leader_profile_id.clone(),
      leader_profile_name: session.leader_profile_name.clone(),
      followers: session.followers.values().cloned().collect(),
    };
    let _ = app_handle.emit("sync-session-changed", &info);

    Ok(())
  }

  /// Get all active sync sessions.
  pub async fn get_sessions(&self) -> Vec<SyncSessionInfo> {
    let inner = self.inner.lock().await;
    inner
      .sessions
      .values()
      .map(|s| SyncSessionInfo {
        id: s.id.clone(),
        leader_profile_id: s.leader_profile_id.clone(),
        leader_profile_name: s.leader_profile_name.clone(),
        followers: s.followers.values().cloned().collect(),
      })
      .collect()
  }

  // --- Helper methods ---

  fn get_profile(profile_id: &str) -> Result<BrowserProfile, String> {
    let profiles = ProfileManager::instance()
      .list_profiles()
      .map_err(|e| format!("Failed to list profiles: {e}"))?;
    profiles
      .into_iter()
      .find(|p| p.id.to_string() == profile_id)
      .ok_or(format!("Profile '{profile_id}' not found"))
  }

  /// Reserve a free loopback port by binding and immediately releasing it.
  ///
  /// There is an inherent race between releasing and the browser binding, but
  /// this is the same approach the automation broker uses and the window is a
  /// few milliseconds on a loopback interface.
  fn reserve_loopback_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
      .map_err(|e| format!("Failed to reserve a loopback CDP port: {e}"))?;
    listener
      .local_addr()
      .map(|addr| addr.port())
      .map_err(|e| format!("Failed to read the reserved CDP port: {e}"))
  }

  /// Look up the CDP port this session assigned to a profile, waiting for the
  /// endpoint to answer. The port comes from the session rather than from the
  /// process, because only browse-together launches these profiles with remote
  /// debugging enabled.
  async fn get_cdp_port(
    manager: &Arc<AsyncMutex<SynchronizerInner>>,
    session_id: &str,
    profile_id: &str,
  ) -> Result<u16, String> {
    let port = {
      let inner = manager.lock().await;
      inner
        .sessions
        .get(session_id)
        .and_then(|s| s.cdp_ports.get(profile_id).copied())
        .ok_or_else(|| format!("No CDP port recorded for profile '{profile_id}'"))?
    };

    // The browser needs a moment after launch before the endpoint accepts.
    for attempt in 0..30 {
      if attempt > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
      }
      if Self::get_page_ws_url(port).await.is_ok() {
        return Ok(port);
      }
    }
    Err(format!(
      "CDP endpoint on port {port} did not become ready for profile '{profile_id}'"
    ))
  }

  /// Get a page-level WebSocket URL
  async fn get_page_ws_url(port: u16) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/json");
    let client = reqwest::Client::new();
    for attempt in 0..15 {
      if attempt > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
      }
      if let Ok(resp) = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
      {
        if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
          if let Some(ws_url) = targets
            .iter()
            .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
            .and_then(|t| t.get("webSocketDebuggerUrl"))
            .and_then(|v| v.as_str())
          {
            return Ok(ws_url.to_string());
          }
        }
      }
    }
    Err("Failed to get CDP WebSocket URL".to_string())
  }
}

// --- Tauri Commands ---

#[tauri::command]
pub async fn start_sync_session(
  app_handle: tauri::AppHandle,
  leader_profile_id: String,
  follower_profile_ids: Vec<String>,
) -> Result<SyncSessionInfo, String> {
  SynchronizerManager::instance()
    .start_session(app_handle, leader_profile_id, follower_profile_ids)
    .await
}

#[tauri::command]
pub async fn stop_sync_session(
  app_handle: tauri::AppHandle,
  session_id: String,
) -> Result<(), String> {
  SynchronizerManager::instance()
    .stop_session(app_handle, &session_id)
    .await
}

#[tauri::command]
pub async fn remove_sync_follower(
  app_handle: tauri::AppHandle,
  session_id: String,
  follower_profile_id: String,
) -> Result<(), String> {
  SynchronizerManager::instance()
    .remove_follower(app_handle, &session_id, &follower_profile_id)
    .await
}

#[tauri::command]
pub async fn get_sync_sessions() -> Result<Vec<SyncSessionInfo>, String> {
  Ok(SynchronizerManager::instance().get_sessions().await)
}

#[cfg(test)]
mod tests {
  use super::*;

  /// The capture script and `CapturedEvent` form a contract across a language
  /// boundary: the script builds these objects in the page, Rust deserializes
  /// them. A renamed field on either side fails silently at runtime — input
  /// would simply stop replaying — so pin the exact shapes the script emits.
  #[test]
  fn captured_event_parses_every_payload_the_script_emits() {
    let mousedown = r#"{"type":"mousedown","x":12.5,"y":30,"button":"left","clickCount":1,"modifiers":8,"timestamp":1700000000000}"#;
    let e: CapturedEvent = serde_json::from_str(mousedown).expect("mousedown");
    assert_eq!(e.event_type, "mousedown");
    assert_eq!(e.x, Some(12.5));
    assert_eq!(e.button.as_deref(), Some("left"));
    assert_eq!(e.click_count, Some(1));
    assert_eq!(e.modifiers, Some(8));

    let wheel = r#"{"type":"wheel","deltaX":0,"deltaY":-120.5,"timestamp":1700000000000}"#;
    let e: CapturedEvent = serde_json::from_str(wheel).expect("wheel");
    assert_eq!(e.delta_y, Some(-120.5));

    let keydown = r#"{"type":"keydown","key":"a","code":"KeyA","windowsVirtualKeyCode":65,"modifiers":0,"timestamp":1}"#;
    let e: CapturedEvent = serde_json::from_str(keydown).expect("keydown");
    assert_eq!(e.key.as_deref(), Some("a"));
    assert_eq!(e.code.as_deref(), Some("KeyA"));
    assert_eq!(e.key_code, Some(65));

    let ch = r#"{"type":"char","text":"a","modifiers":0,"timestamp":1}"#;
    let e: CapturedEvent = serde_json::from_str(ch).expect("char");
    assert_eq!(e.text.as_deref(), Some("a"));
  }

  #[test]
  fn capture_binding_call_is_recognised() {
    let ours = serde_json::json!({
      "method": "Runtime.bindingCalled",
      "params": { "name": CAPTURE_BINDING, "payload": "{}" }
    });
    assert!(SynchronizerManager::is_capture_binding_call(&ours));
  }

  #[test]
  fn other_bindings_and_events_are_ignored() {
    // A different binding on the same event must not be treated as input.
    let other_binding = serde_json::json!({
      "method": "Runtime.bindingCalled",
      "params": { "name": "somethingElse", "payload": "{}" }
    });
    assert!(!SynchronizerManager::is_capture_binding_call(
      &other_binding
    ));

    // Navigation events flow through the same handler and must not match.
    let nav = serde_json::json!({
      "method": "Page.frameNavigated",
      "params": { "frame": { "url": "https://example.com" } }
    });
    assert!(!SynchronizerManager::is_capture_binding_call(&nav));

    // Malformed messages must not panic or match.
    assert!(!SynchronizerManager::is_capture_binding_call(
      &serde_json::json!({})
    ));
  }

  #[test]
  fn capture_script_targets_the_isolated_world_binding() {
    // The script calls the binding by literal name; if the constant is renamed
    // without updating the script the two silently stop matching.
    assert!(
      CAPTURE_SCRIPT.contains(CAPTURE_BINDING),
      "capture script must call the binding it is registered under"
    );
    // mousemove is deliberately never emitted: replaying it floods followers.
    assert!(
      !CAPTURE_SCRIPT.contains("\"mousemove\""),
      "the script must not emit mousemove"
    );
  }

  #[test]
  fn reserved_loopback_port_is_usable() {
    let port = SynchronizerManager::reserve_loopback_port().expect("a port is available");
    assert!(port > 0);
    // The port is released on return, so it must be bindable again.
    std::net::TcpListener::bind(("127.0.0.1", port)).expect("released port rebinds");
  }
}
