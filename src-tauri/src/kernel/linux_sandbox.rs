//! Preflight for Chromium's Linux sandbox.
//!
//! A downloaded Chromium can only sandbox itself two ways: a setuid-root
//! `chrome-sandbox` helper next to the binary, or unprivileged user
//! namespaces. Distributions increasingly ship with userns restricted
//! (Ubuntu 24.04's AppArmor rule, hardened sysctls), and a kernel unpacked
//! into the user's data directory has no setuid helper, because setting that
//! bit needs root. The launch then dies instantly with a namespace error the
//! user never sees. Detect the situation up front and report it as a code the
//! UI can explain, rather than spawning a process that cannot survive.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxReadiness {
  Ready,
  /// No setuid helper and the host forbids unprivileged user namespaces.
  UserNamespacesRestricted,
}

/// Decide readiness from already-read host facts. Split out from the probing
/// so the rule is testable on any host.
pub fn readiness_from(
  setuid_sandbox_present: bool,
  apparmor_restrict_unprivileged_userns: Option<u64>,
  unprivileged_userns_clone: Option<u64>,
  max_user_namespaces: Option<u64>,
) -> SandboxReadiness {
  if setuid_sandbox_present {
    return SandboxReadiness::Ready;
  }
  if apparmor_restrict_unprivileged_userns == Some(1)
    || unprivileged_userns_clone == Some(0)
    || max_user_namespaces == Some(0)
  {
    return SandboxReadiness::UserNamespacesRestricted;
  }
  SandboxReadiness::Ready
}

#[cfg(target_os = "linux")]
pub fn check_install(install_root: &Path) -> SandboxReadiness {
  readiness_from(
    has_setuid_sandbox(install_root),
    read_flag("/proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
    read_flag("/proc/sys/kernel/unprivileged_userns_clone"),
    read_flag("/proc/sys/user/max_user_namespaces"),
  )
}

/// Every other host sandboxes without a helper of its own.
#[cfg(not(target_os = "linux"))]
pub fn check_install(_install_root: &Path) -> SandboxReadiness {
  SandboxReadiness::Ready
}

#[cfg(target_os = "linux")]
fn read_flag(path: &str) -> Option<u64> {
  std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn has_setuid_sandbox(install_root: &Path) -> bool {
  use std::os::unix::fs::MetadataExt;
  use std::os::unix::fs::PermissionsExt;

  ["chrome-sandbox", "chrome_sandbox"]
    .iter()
    .filter_map(|name| std::fs::metadata(install_root.join(name)).ok())
    .any(|meta| meta.permissions().mode() & 0o4000 != 0 && meta.uid() == 0)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_setuid_helper_settles_it() {
    assert_eq!(
      readiness_from(true, Some(1), Some(0), Some(0)),
      SandboxReadiness::Ready
    );
  }

  #[test]
  fn restricted_user_namespaces_block_the_launch() {
    for (apparmor, clone, max) in [
      (Some(1), None, None),
      (None, Some(0), None),
      (None, None, Some(0)),
    ] {
      assert_eq!(
        readiness_from(false, apparmor, clone, max),
        SandboxReadiness::UserNamespacesRestricted
      );
    }
  }

  #[test]
  fn an_unrestricted_host_needs_no_helper() {
    assert_eq!(
      readiness_from(false, Some(0), Some(1), Some(10000)),
      SandboxReadiness::Ready
    );
    // Files absent (non-AppArmor distros) reads as unrestricted.
    assert_eq!(
      readiness_from(false, None, None, None),
      SandboxReadiness::Ready
    );
  }

  #[cfg(not(target_os = "linux"))]
  #[test]
  fn other_hosts_never_gate_a_launch() {
    assert_eq!(
      check_install(Path::new("/nonexistent")),
      SandboxReadiness::Ready
    );
  }
}
