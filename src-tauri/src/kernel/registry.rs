//! Resolve a KernelDriver by profile.browser / kernel id string.

use super::driver::KernelDriver;
use super::fingerprint_chromium::FingerprintChromiumDriver;
use std::collections::HashMap;
use std::sync::Arc;

pub struct KernelRegistry {
  drivers: HashMap<&'static str, Arc<dyn KernelDriver>>,
}

impl KernelRegistry {
  pub fn new() -> Self {
    let mut drivers: HashMap<&'static str, Arc<dyn KernelDriver>> = HashMap::new();
    drivers.insert(
      "fingerprint-chromium",
      Arc::new(FingerprintChromiumDriver::new()),
    );
    Self { drivers }
  }

  pub fn instance() -> &'static KernelRegistry {
    &KERNEL_REGISTRY
  }

  pub fn get(&self, kernel_id: &str) -> Option<Arc<dyn KernelDriver>> {
    self.drivers.get(kernel_id).map(Arc::clone)
  }

  pub fn require(&self, kernel_id: &str) -> Result<Arc<dyn KernelDriver>, String> {
    self
      .get(kernel_id)
      .ok_or_else(|| format!("Unknown kernel: {kernel_id}"))
  }

  pub fn ids(&self) -> Vec<&'static str> {
    let mut ids: Vec<_> = self.drivers.keys().copied().collect();
    ids.sort_unstable();
    ids
  }
}

impl Default for KernelRegistry {
  fn default() -> Self {
    Self::new()
  }
}

lazy_static::lazy_static! {
  static ref KERNEL_REGISTRY: KernelRegistry = KernelRegistry::new();
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolves_only_the_supported_kernel() {
    let reg = KernelRegistry::new();
    assert_eq!(
      reg.require("fingerprint-chromium").unwrap().id(),
      "fingerprint-chromium"
    );
    assert!(reg.require("unknown").is_err());
    // The legacy engine was removed; its id must no longer resolve.
    assert!(reg.require("wayfern").is_err());
  }
}
