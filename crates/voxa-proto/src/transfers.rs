//! File payload budgets, separate from voice, chat and stream traffic.
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransferLimits {
    pub upload_kib: u32,
    pub download_kib: u32,
    pub uploads: u16,
    pub downloads: u16,
}

impl TransferLimits {
    pub fn valid(self) -> bool {
        self.upload_kib <= 1_000_000 && self.download_kib <= 1_000_000 && self.uploads <= 1024 && self.downloads <= 1024
    }
}

/// One shared clock per direction, not one quota per file. Waiting for the async
/// mutex is FIFO. Cancellation releases it even in the middle of a chunk.
pub struct Rate {
    kib: AtomicU32,
    clock: tokio::sync::Mutex<Instant>,
}

impl Rate {
    pub fn new(kib: u32) -> Self {
        Self { kib: kib.into(), clock: tokio::sync::Mutex::new(Instant::now()) }
    }

    pub fn set(&self, kib: u32) {
        self.kib.store(kib, Ordering::Relaxed);
    }

    pub fn chunk(&self) -> usize {
        match self.kib.load(Ordering::Relaxed) {
            0 => crate::FILE_CHUNK,
            rate => ((u64::from(rate) * 1024 / 8) as usize).clamp(128, crate::FILE_CHUNK),
        }
    }

    pub async fn pace(&self, bytes: usize) {
        self.pace_while(bytes, || true).await;
    }

    pub async fn pace_while(&self, bytes: usize, active: impl Fn() -> bool) -> bool {
        let mut remaining = bytes as f64;
        while remaining > 0.5 {
            if !active() {
                return false;
            }
            let rate = f64::from(self.kib.load(Ordering::Relaxed)) * 1024.0;
            if rate == 0.0 {
                break;
            }
            // Take turns at most every 125 ms, even if a legacy client supplied
            // a large chunk while the administrator configured a very low rate.
            let mut clock = self.clock.lock().await;
            let now = Instant::now();
            // No accumulated idle credit beyond one small burst.
            let credit = now.saturating_duration_since(*clock).as_secs_f64().min(0.125) * rate;
            remaining -= credit;
            *clock = now;
            if remaining <= 0.5 {
                break;
            }
            let delay = Duration::from_secs_f64((remaining / rate).min(0.125));
            tokio::time::sleep(delay).await;
            // Consume only once, including on slow timer wakes.
            let awake = Instant::now();
            remaining -= awake.duration_since(now).as_secs_f64() * rate;
            *clock = awake;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_shared_rate_is_aggregate_and_unlimited_bypasses_waits() {
        let rate = Rate::new(64);
        let start = Instant::now();
        tokio::join!(rate.pace(8192), rate.pace(8192));
        assert!(start.elapsed() >= Duration::from_millis(230));
        rate.set(0);
        tokio::time::timeout(Duration::from_millis(100), rate.pace(crate::FILE_CHUNK)).await.unwrap();
        assert_eq!(rate.chunk(), crate::FILE_CHUNK);
    }
}
