use recovery_core::carve::signatures::SignatureLibrary;
use recovery_core::device::BlockDevice;
use recovery_core::model::FoundFile;
use recovery_core::progress::{Progress, SharedProgress};
use recovery_core::recover::RecoverReport;
use recovery_core::session::{SessionMeta, SessionStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The app data dir Tauri will use (`%APPDATA%\<identifier>` on Windows,
/// `~/Library/Application Support/<identifier>` on macOS). Computed without a
/// Tauri handle so it can run before the webview is created.
pub fn app_data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }?;
    Some(base.join("com.mjgroup.jfilerescue"))
}

/// Results-store root the user chose, if any (see `AppState::set_store_root`).
pub fn persisted_store_root() -> Option<PathBuf> {
    let p = app_data_dir()?.join("store_root.txt");
    let s = std::fs::read_to_string(p).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

pub struct ActiveScan {
    pub session_id: String,
    pub progress: SharedProgress,
    pub source_path: String,
    pub done: Arc<Mutex<Option<Result<(), String>>>>,
    pub memory_only: bool,
    /// Finished memory-only result, picked up by `scan_progress`.
    pub pending: Arc<Mutex<Option<MemorySession>>>,
}

/// Results of a scan whose store was NOT allowed to touch disk (memory-only
/// session). Lives until the user picks a results location on another disk or
/// discards it.
pub struct MemorySession {
    pub meta: SessionMeta,
    pub files: Arc<Vec<FoundFile>>,
}

pub struct ActiveRecover {
    pub progress: SharedProgress,
    pub result: Arc<Mutex<Option<Result<RecoverReport, String>>>>,
    #[allow(dead_code)]
    pub partial: Arc<Mutex<Vec<recovery_core::recover::ItemResult>>>,
}

pub struct AppState {
    pub data_dir: PathBuf,
    /// Where sessions are written. Defaults to `data_dir/sessions`; the user can
    /// move it to another disk (persisted in `data_dir/store_root.txt`).
    pub store: Mutex<SessionStore>,
    pub sig_path: PathBuf,
    pub sigs: Mutex<SignatureLibrary>,
    pub platform: Box<dyn recovery_platform::Platform + Send + Sync>,
    pub scan: Mutex<Option<ActiveScan>>,
    pub recover: Mutex<Option<ActiveRecover>>,
    /// Cached loaded session files for fast querying.
    pub cache: Mutex<HashMap<String, Arc<Vec<FoundFile>>>>,
    /// Memory-only sessions (store on source disk → never flushed).
    pub memory: Mutex<HashMap<String, MemorySession>>,
    /// Open devices by path (read-only handles reused across preview calls).
    pub devices: Mutex<HashMap<String, Arc<dyn BlockDevice>>>,
    /// Volumes currently locked by a running scan (recovery refused).
    pub locked_sources: Mutex<Vec<String>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let sig_path = data_dir.join("signatures.toml");
        let sigs = SignatureLibrary::load(&sig_path).unwrap_or_else(|e| {
            log::warn!("signature overrides unreadable, using builtin: {}", e);
            SignatureLibrary::builtin()
        });
        let store_root = persisted_store_root().unwrap_or_else(|| data_dir.join("sessions"));
        Self {
            store: Mutex::new(SessionStore::new(store_root)),
            sig_path,
            sigs: Mutex::new(sigs),
            platform: recovery_platform::current(),
            scan: Mutex::new(None),
            recover: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            memory: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            locked_sources: Mutex::new(vec![]),
            data_dir,
        }
    }

    pub fn store_root(&self) -> PathBuf {
        self.store.lock().unwrap().root().to_path_buf()
    }

    /// Persist a new results location and switch to it.
    pub fn set_store_root(&self, root: PathBuf) -> Result<(), String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        std::fs::write(
            self.data_dir.join("store_root.txt"),
            root.to_string_lossy().as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        *self.store.lock().unwrap() = SessionStore::new(root);
        self.cache.lock().unwrap().clear();
        Ok(())
    }

    /// Disk identity of the results store (where sessions get written).
    pub fn store_disk_identity(&self) -> Option<String> {
        let root = self.store_root();
        let probe = root
            .ancestors()
            .find(|a| a.exists())
            .map(|a| a.to_path_buf())
            .unwrap_or(root);
        self.platform.disk_identity_of_path(&probe)
    }

    /// Would writing session results touch the disk being scanned?
    pub fn store_on_source(&self, source_path: &str) -> bool {
        recovery_platform::same_disk(self.platform.as_ref(), source_path, &self.store_root())
    }

    pub fn files_of(&self, id: &str) -> Result<Arc<Vec<FoundFile>>, String> {
        if let Some(f) = self.cache.lock().unwrap().get(id) {
            return Ok(Arc::clone(f));
        }
        if let Some(m) = self.memory.lock().unwrap().get(id) {
            return Ok(Arc::clone(&m.files));
        }
        let files = Arc::new(
            self.store
                .lock()
                .unwrap()
                .load_files(id)
                .map_err(|e| e.to_string())?,
        );
        self.cache
            .lock()
            .unwrap()
            .insert(id.to_string(), Arc::clone(&files));
        Ok(files)
    }

    pub fn device(&self, path: &str) -> Result<Arc<dyn BlockDevice>, String> {
        if let Some(d) = self.devices.lock().unwrap().get(path) {
            return Ok(Arc::clone(d));
        }
        let d = recovery_platform::open_source(self.platform.as_ref(), path)
            .map_err(|e| e.to_string())?;
        self.devices
            .lock()
            .unwrap()
            .insert(path.to_string(), Arc::clone(&d));
        Ok(d)
    }

    pub fn meta(&self, id: &str) -> Result<SessionMeta, String> {
        if let Some(m) = self.memory.lock().unwrap().get(id) {
            return Ok(m.meta.clone());
        }
        self.store
            .lock()
            .unwrap()
            .load_meta(id)
            .map_err(|e| e.to_string())
    }

    pub fn new_progress() -> SharedProgress {
        Progress::new()
    }
}
