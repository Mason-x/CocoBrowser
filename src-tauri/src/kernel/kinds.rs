//! Stable kernel identifiers and compatibility predicates.

pub const FINGERPRINT_CHROMIUM: &str = "fingerprint-chromium";
pub const CLOAK_BROWSER_146: &str = "cloakbrowser-146";
pub const CLOAK_BROWSER_150: &str = "cloakbrowser-150";

pub fn is_cloak_browser(id: &str) -> bool {
  matches!(id, CLOAK_BROWSER_146 | CLOAK_BROWSER_150)
}

pub fn is_persona_kernel(id: &str) -> bool {
  matches!(
    id,
    FINGERPRINT_CHROMIUM | CLOAK_BROWSER_146 | CLOAK_BROWSER_150
  )
}

pub fn is_creatable_kernel(id: &str) -> bool {
  is_cloak_browser(id)
}

pub fn requires_cloak_license(id: &str) -> bool {
  id == CLOAK_BROWSER_150
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classifies_current_and_legacy_kernels() {
    assert!(is_creatable_kernel(CLOAK_BROWSER_146));
    assert!(is_creatable_kernel(CLOAK_BROWSER_150));
    assert!(is_persona_kernel(FINGERPRINT_CHROMIUM));
    assert!(!is_creatable_kernel(FINGERPRINT_CHROMIUM));
    assert!(requires_cloak_license(CLOAK_BROWSER_150));
    assert!(!requires_cloak_license(CLOAK_BROWSER_146));
  }
}
