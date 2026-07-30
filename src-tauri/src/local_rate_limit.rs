//! In-memory limits for the loopback automation servers.
//!
//! These are local product policy and are intentionally independent of cloud
//! subscriptions or commercial entitlements.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const LOCAL_REQUESTS_PER_HOUR: u32 = 3_600;

#[derive(Debug)]
struct Window {
  started: Instant,
  requests: u32,
}

#[derive(Debug)]
pub struct FixedWindowLimiter {
  limit: u32,
  period: Duration,
  window: Mutex<Window>,
}

impl FixedWindowLimiter {
  pub fn new(limit: u32, period: Duration) -> Self {
    Self {
      limit,
      period,
      window: Mutex::new(Window {
        started: Instant::now(),
        requests: 0,
      }),
    }
  }

  pub fn try_acquire(&self) -> bool {
    let Ok(mut window) = self.window.lock() else {
      // A poisoned limiter must not silently remove a protection boundary.
      return false;
    };

    if window.started.elapsed() >= self.period {
      window.started = Instant::now();
      window.requests = 0;
    }
    if window.requests >= self.limit {
      return false;
    }
    window.requests += 1;
    true
  }
}

lazy_static::lazy_static! {
  pub static ref API_RATE_LIMITER: FixedWindowLimiter = FixedWindowLimiter::new(
    LOCAL_REQUESTS_PER_HOUR,
    Duration::from_secs(60 * 60),
  );
  pub static ref MCP_RATE_LIMITER: FixedWindowLimiter = FixedWindowLimiter::new(
    LOCAL_REQUESTS_PER_HOUR,
    Duration::from_secs(60 * 60),
  );
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn blocks_after_limit() {
    let limiter = FixedWindowLimiter::new(2, Duration::from_secs(60));
    assert!(limiter.try_acquire());
    assert!(limiter.try_acquire());
    assert!(!limiter.try_acquire());
  }

  #[test]
  fn resets_after_window() {
    let limiter = FixedWindowLimiter::new(1, Duration::ZERO);
    assert!(limiter.try_acquire());
    assert!(limiter.try_acquire());
  }
}
