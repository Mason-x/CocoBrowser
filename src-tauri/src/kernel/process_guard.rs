//! Process tree ownership: Windows Job Objects; Unix process-group fallback.
//!
//! Never kill-by-name. Only terminate processes we started and track by
//! (pid, creation time) and/or Job Object membership.

use std::time::SystemTime;
use std::{collections::BTreeMap, path::Path};

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Process tree ownership handle. Stored as raw integers so the type is `Send`
/// and can cross `await` points in async launchers.
#[derive(Debug)]
pub struct ProcessGuard {
  pub pid: u32,
  pub created_at: SystemTime,
  #[cfg(target_os = "windows")]
  job_raw: Option<isize>,
  #[cfg(target_os = "windows")]
  job_name: Option<String>,
  /// Keep child handle alive so we can query / assign; dropped on stop.
  child: Option<std::process::Child>,
}

// Child and raw HANDLE are only used on the owning thread of the job; we
// still mark Send so SessionManager async launch can hold the guard briefly.
unsafe impl Send for ProcessGuard {}

impl ProcessGuard {
  /// Spawn `exe` with `args`, attach to a Job Object on Windows so the whole
  /// Chromium process tree is killed together.
  pub fn spawn(exe: &Path, args: &[String]) -> Result<Self, String> {
    Self::spawn_with_env(exe, args, &BTreeMap::new())
  }

  /// Spawn with a small set of additional environment variables. The parent
  /// environment remains inherited; only the supplied keys are overlaid.
  pub fn spawn_with_env(
    exe: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
  ) -> Result<Self, String> {
    let mut cmd = std::process::Command::new(exe);
    cmd
      .args(args)
      .envs(env)
      .stdin(std::process::Stdio::null())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
      // CREATE_BREAKAWAY_FROM_JOB not set — children stay in our job when assigned.
      // CREATE_NEW_PROCESS_GROUP for cleaner signaling.
      const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
      cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    #[cfg(unix)]
    {
      use std::os::unix::process::CommandExt;
      // New session/process group so we can kill the tree with -pid.
      unsafe {
        cmd.pre_exec(|| {
          if libc::setsid() == -1 {
            return Err(std::io::Error::last_os_error());
          }
          Ok(())
        });
      }
    }

    #[cfg(target_os = "windows")]
    let (job_name, job) = {
      let name = format!("Local\\lfb-job-{}", uuid::Uuid::new_v4());
      let job = create_kill_on_close_job(&name)?;
      (name, job)
    };

    let mut child = match cmd.spawn() {
      Ok(child) => child,
      Err(error) => {
        #[cfg(target_os = "windows")]
        unsafe {
          let _ = windows::Win32::Foundation::CloseHandle(job);
        }
        return Err(format!("failed to spawn {}: {error}", exe.display()));
      }
    };
    let pid = child.id();
    let created_at = SystemTime::now();

    #[cfg(target_os = "windows")]
    {
      if let Err(error) = assign_process_to_job(job, &child) {
        let _ = child.kill();
        let _ = child.wait();
        unsafe {
          let _ = windows::Win32::Foundation::CloseHandle(job);
        }
        return Err(error);
      }
      Ok(Self {
        pid,
        created_at,
        job_raw: Some(job.0 as isize),
        job_name: Some(job_name),
        child: Some(child),
      })
    }

    #[cfg(not(target_os = "windows"))]
    {
      Ok(Self {
        pid,
        created_at,
        child: Some(child),
      })
    }
  }

  pub fn job_token(&self) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
      self.job_name.clone()
    }
    #[cfg(not(target_os = "windows"))]
    {
      None
    }
  }

  /// Terminate only this process tree (Job Object or process group).
  pub fn terminate(mut self) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
      if let Some(raw) = self.job_raw.take() {
        // SAFETY: job handle owned by us; TerminateJobObject kills all members.
        unsafe {
          use windows::Win32::Foundation::HANDLE;
          use windows::Win32::System::JobObjects::TerminateJobObject;
          let job = HANDLE(raw as *mut _);
          let _ = TerminateJobObject(job, 1);
          let _ = windows::Win32::Foundation::CloseHandle(job);
        }
      } else if let Some(mut child) = self.child.take() {
        let _ = child.kill();
        let _ = child.wait();
      }
      Ok(())
    }

    #[cfg(unix)]
    {
      // Negative pid = process group.
      unsafe {
        let _ = libc::kill(-(self.pid as i32), libc::SIGTERM);
      }
      if let Some(mut child) = self.child.take() {
        // Best-effort wait; don't block forever.
        let _ = child.try_wait();
      }
      Ok(())
    }

    #[cfg(not(any(target_os = "windows", unix)))]
    {
      if let Some(mut child) = self.child.take() {
        let _ = child.kill();
      }
      Ok(())
    }
  }

  pub fn is_alive(&mut self) -> bool {
    matches!(self.try_exit_code(), Ok(None))
  }

  pub fn try_exit_code(&mut self) -> Result<Option<i32>, String> {
    let Some(child) = self.child.as_mut() else {
      return Ok(Some(-1));
    };
    child
      .try_wait()
      .map(|status| status.map(|value| value.code().unwrap_or(-1)))
      .map_err(|e| e.to_string())
  }
}

impl Drop for ProcessGuard {
  fn drop(&mut self) {
    // If caller forgot terminate(), still tear down the tree.
    #[cfg(target_os = "windows")]
    {
      if let Some(raw) = self.job_raw.take() {
        unsafe {
          use windows::Win32::Foundation::HANDLE;
          use windows::Win32::System::JobObjects::TerminateJobObject;
          let job = HANDLE(raw as *mut _);
          let _ = TerminateJobObject(job, 1);
          let _ = windows::Win32::Foundation::CloseHandle(job);
        }
      }
    }
    if let Some(mut child) = self.child.take() {
      let _ = child.kill();
    }
  }
}

#[cfg(target_os = "windows")]
fn create_kill_on_close_job(name: &str) -> Result<windows::Win32::Foundation::HANDLE, String> {
  use std::os::windows::ffi::OsStrExt;
  use windows::core::PCWSTR;
  use windows::Win32::Foundation::CloseHandle;
  use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
  };

  let wide: Vec<u16> = std::ffi::OsStr::new(name)
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();

  let job = unsafe { CreateJobObjectW(None, PCWSTR(wide.as_ptr())) }
    .map_err(|e| format!("CreateJobObjectW: {e}"))?;

  let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
  info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

  let ok = unsafe {
    SetInformationJobObject(
      job,
      JobObjectExtendedLimitInformation,
      &info as *const _ as *const _,
      std::mem::size_of_val(&info) as u32,
    )
  };
  if ok.is_err() {
    unsafe {
      let _ = CloseHandle(job);
    }
    return Err("SetInformationJobObject KILL_ON_JOB_CLOSE failed".into());
  }
  Ok(job)
}

#[cfg(target_os = "windows")]
fn assign_process_to_job(
  job: windows::Win32::Foundation::HANDLE,
  child: &std::process::Child,
) -> Result<(), String> {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::System::JobObjects::AssignProcessToJobObject;

  // Child's raw handle is a process handle.
  let process = HANDLE(child.as_raw_handle() as *mut _);
  unsafe { AssignProcessToJobObject(job, process) }
    .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn spawn_and_terminate_self_only() {
    // Use a short-lived process that stays alive until killed.
    #[cfg(target_os = "windows")]
    let (exe, args) = {
      (
        std::path::PathBuf::from("cmd.exe"),
        vec![
          "/C".into(),
          "ping".into(),
          "-n".into(),
          "60".into(),
          "127.0.0.1".into(),
        ],
      )
    };
    #[cfg(not(target_os = "windows"))]
    let (exe, args) = { (std::path::PathBuf::from("sleep"), vec!["30".into()]) };

    let mut guard = ProcessGuard::spawn(&exe, &args).expect("spawn");
    assert!(guard.pid > 0);
    assert!(guard.is_alive());
    guard.terminate().expect("terminate");
  }
}
