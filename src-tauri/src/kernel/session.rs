//! Profile launch session state machine and single-instance locks.
//!
//! ```text
//! Stopped -> PreparingProxy -> ValidatingPersona -> Launching
//!         -> WaitingForReady -> Running -> Stopping -> Stopped
//! any failure -> Error -> cleanup
//! ```

use super::launch_plan::BrowserProcess;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
  Stopped,
  PreparingProxy,
  ValidatingPersona,
  Launching,
  WaitingForReady,
  Running,
  Stopping,
  Error,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
  pub profile_id: Uuid,
  pub state: SessionState,
  pub process: Option<BrowserProcess>,
  pub pid: Option<u32>,
  pub created_at: Option<SystemTime>,
  pub error: Option<String>,
  /// Opaque OS job / process-group handle token (Windows Job Object name).
  pub job_token: Option<String>,
}

impl SessionRecord {
  fn new(profile_id: Uuid) -> Self {
    Self {
      profile_id,
      state: SessionState::Stopped,
      process: None,
      pid: None,
      created_at: None,
      error: None,
      job_token: None,
    }
  }
}

pub struct SessionManager {
  sessions: Mutex<HashMap<Uuid, SessionRecord>>,
}

impl SessionManager {
  fn new() -> Self {
    Self {
      sessions: Mutex::new(HashMap::new()),
    }
  }

  pub fn instance() -> &'static SessionManager {
    &SESSION_MANAGER
  }

  /// Begin a launch. Fails if the profile is already launching or running.
  pub fn try_begin_launch(&self, profile_id: Uuid) -> Result<(), String> {
    let mut map = self
      .sessions
      .lock()
      .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(existing) = map.get(&profile_id) {
      match existing.state {
        SessionState::Stopped | SessionState::Error => {}
        other => {
          return Err(format!(
            "profile {profile_id} already has an active session ({other:?})"
          ));
        }
      }
    }
    let mut rec = SessionRecord::new(profile_id);
    rec.state = SessionState::PreparingProxy;
    map.insert(profile_id, rec);
    Ok(())
  }

  pub fn set_state(&self, profile_id: Uuid, state: SessionState) -> Result<(), String> {
    let mut map = self
      .sessions
      .lock()
      .map_err(|_| "session lock poisoned".to_string())?;
    let rec = map
      .get_mut(&profile_id)
      .ok_or_else(|| format!("no session for {profile_id}"))?;
    rec.state = state;
    if state == SessionState::Stopped {
      rec.process = None;
      rec.pid = None;
      rec.created_at = None;
      rec.job_token = None;
      rec.error = None;
    }
    Ok(())
  }

  pub fn set_error(&self, profile_id: Uuid, error: impl Into<String>) -> Result<(), String> {
    let mut map = self
      .sessions
      .lock()
      .map_err(|_| "session lock poisoned".to_string())?;
    let rec = map
      .get_mut(&profile_id)
      .ok_or_else(|| format!("no session for {profile_id}"))?;
    rec.state = SessionState::Error;
    rec.error = Some(error.into());
    Ok(())
  }

  pub fn mark_running(
    &self,
    profile_id: Uuid,
    process: BrowserProcess,
    job_token: Option<String>,
  ) -> Result<(), String> {
    let mut map = self
      .sessions
      .lock()
      .map_err(|_| "session lock poisoned".to_string())?;
    let rec = map
      .get_mut(&profile_id)
      .ok_or_else(|| format!("no session for {profile_id}"))?;
    rec.pid = process.pid;
    rec.created_at = process.created_at;
    rec.job_token = job_token;
    rec.process = Some(process);
    rec.state = SessionState::Running;
    rec.error = None;
    Ok(())
  }

  pub fn get(&self, profile_id: Uuid) -> Option<SessionRecord> {
    self
      .sessions
      .lock()
      .ok()
      .and_then(|m| m.get(&profile_id).cloned())
  }

  pub fn is_running(&self, profile_id: Uuid) -> bool {
    matches!(
      self.get(profile_id).map(|s| s.state),
      Some(SessionState::Running)
        | Some(SessionState::Launching)
        | Some(SessionState::WaitingForReady)
        | Some(SessionState::PreparingProxy)
        | Some(SessionState::ValidatingPersona)
    )
  }

  /// Clear session after stop (or failed cleanup).
  pub fn end(&self, profile_id: Uuid) {
    if let Ok(mut map) = self.sessions.lock() {
      map.remove(&profile_id);
    }
  }
}

lazy_static::lazy_static! {
  static ref SESSION_MANAGER: SessionManager = SessionManager::new();
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rejects_double_launch() {
    let mgr = SessionManager::new();
    let id = Uuid::new_v4();
    mgr.try_begin_launch(id).unwrap();
    assert!(mgr.try_begin_launch(id).is_err());
    mgr.set_state(id, SessionState::Stopped).unwrap();
    mgr.try_begin_launch(id).unwrap();
  }

  #[test]
  fn state_transitions_to_running() {
    let mgr = SessionManager::new();
    let id = Uuid::new_v4();
    mgr.try_begin_launch(id).unwrap();
    mgr.set_state(id, SessionState::ValidatingPersona).unwrap();
    mgr.set_state(id, SessionState::Launching).unwrap();
    let proc = BrowserProcess {
      profile_id: id.to_string(),
      kernel_id: "fingerprint-chromium".into(),
      pid: Some(1234),
      created_at: Some(SystemTime::now()),
      cdp_port: None,
      user_data_dir: std::path::PathBuf::from("/tmp/p"),
      instance_id: Some("inst".into()),
      used_fingerprint: None,
    };
    mgr.mark_running(id, proc, Some("job-1".into())).unwrap();
    let rec = mgr.get(id).unwrap();
    assert_eq!(rec.state, SessionState::Running);
    assert_eq!(rec.pid, Some(1234));
    mgr.end(id);
    assert!(mgr.get(id).is_none());
  }
}
