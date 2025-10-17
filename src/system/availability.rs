use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct RetryableAvailability {
    available: AtomicBool,
    retry_after: AtomicU64,
    backoff_secs: u64,
}

impl RetryableAvailability {
    pub fn new(initially_available: bool, backoff_secs: u64) -> Self {
        Self {
            available: AtomicBool::new(initially_available),
            retry_after: AtomicU64::new(0),
            backoff_secs,
        }
    }

    pub fn current(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    pub fn try_acquire(&self) -> (bool, bool) {
        if self.available.load(Ordering::Relaxed) {
            return (true, false);
        }

        let retry_after = self.retry_after.load(Ordering::Relaxed);
        if retry_after == 0 {
            return (false, false);
        }

        if now_secs() >= retry_after {
            let became_available = self.mark_available();
            return (true, became_available);
        }

        (false, false)
    }

    pub fn mark_available(&self) -> bool {
        let was_available = self.available.swap(true, Ordering::Relaxed);
        self.retry_after.store(0, Ordering::Relaxed);
        !was_available
    }

    pub fn mark_unavailable(&self) -> bool {
        let was_available = self.available.swap(false, Ordering::Relaxed);
        let retry_at = now_secs().saturating_add(self.backoff_secs);
        self.retry_after.store(retry_at, Ordering::Relaxed);
        was_available
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
