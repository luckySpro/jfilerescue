//! Progress reporting and cooperative cancellation / pause.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Boot,
    Metadata,
    Carve,
    Finalize,
    Done,
    Cancelled,
    Failed,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            0 => Phase::Idle,
            1 => Phase::Boot,
            2 => Phase::Metadata,
            3 => Phase::Carve,
            4 => Phase::Finalize,
            5 => Phase::Done,
            6 => Phase::Cancelled,
            _ => Phase::Failed,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            Phase::Idle => 0,
            Phase::Boot => 1,
            Phase::Metadata => 2,
            Phase::Carve => 3,
            Phase::Finalize => 4,
            Phase::Done => 5,
            Phase::Cancelled => 6,
            Phase::Failed => 7,
        }
    }
}

/// Snapshot for the UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    pub found: u64,
    pub bytes_read: u64,
    pub read_errors: u64,
    pub paused: bool,
    pub message: String,
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Idle
    }
}

/// Shared between the scan thread and whoever observes it.
#[derive(Debug, Default)]
pub struct Progress {
    phase: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    found: AtomicU64,
    bytes_read: AtomicU64,
    read_errors: AtomicU64,
    cancel: AtomicBool,
    pause: AtomicBool,
    message: std::sync::Mutex<String>,
}

pub type SharedProgress = Arc<Progress>;

impl Progress {
    pub fn new() -> SharedProgress {
        Arc::new(Self::default())
    }

    pub fn set_phase(&self, p: Phase, total: u64) {
        self.phase.store(p.as_u8(), Ordering::SeqCst);
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
    }
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::SeqCst))
    }
    pub fn add_done(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
    pub fn set_done(&self, n: u64) {
        self.done.store(n, Ordering::Relaxed);
    }
    pub fn add_found(&self, n: u64) {
        self.found.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_bytes(&self, n: u64) {
        self.bytes_read.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_read_error(&self) {
        self.read_errors.fetch_add(1, Ordering::Relaxed);
    }
    pub fn set_message(&self, m: impl Into<String>) {
        if let Ok(mut g) = self.message.lock() {
            *g = m.into();
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.pause.store(false, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    pub fn set_paused(&self, v: bool) {
        self.pause.store(v, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    /// Blocks while paused; returns `Err(Cancelled)` when cancelled.
    pub fn checkpoint(&self) -> crate::Result<()> {
        loop {
            if self.is_cancelled() {
                return Err(crate::Error::Cancelled);
            }
            if !self.is_paused() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            phase: self.phase(),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            found: self.found.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            read_errors: self.read_errors.load(Ordering::Relaxed),
            paused: self.is_paused(),
            message: self.message.lock().map(|g| g.clone()).unwrap_or_default(),
        }
    }
}
