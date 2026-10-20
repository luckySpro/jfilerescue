use recovery_core::carve::signatures::SignatureLibrary;
use recovery_core::device::BlockDevice;
use recovery_core::model::FoundFile;
use recovery_core::progress::{Progress, SharedProgress};
use recovery_core::recover::RecoverReport;
use recovery_core::session::{SessionMeta, SessionStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct ActiveScan {
    pub session_id: String,
    pub progress: SharedProgress,
    pub source_path: String,
    pub done: Arc<Mutex<Option<Result<(), String>>>>,
}

pub struct ActiveRecover {
    pub progress: SharedProgress,
    pub result: Arc<Mutex<Option<Result<RecoverReport, String>>>>,
    pub partial: Arc<Mutex<Vec<recovery_core::recover::ItemResult>>>,
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub store: SessionStore,
    pub sig_path: PathBuf,
    pub sigs: Mutex<SignatureLibrary>,
    pub platform: Box<dyn recovery_platform::Platform + Send + Sync>,
    pub scan: Mutex<Option<ActiveScan>>,
    pub recover: Mutex<Option<ActiveRecover>>,
    /// Cached loaded session files for fast querying.
    pub cache: Mutex<HashMap<String, Arc<Vec<FoundFile>>>>,
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
        Self {
            store: SessionStore::new(data_dir.join("sessions")),
            sig_path,
            sigs: Mutex::new(sigs),
            platform: recovery_platform::current(),
            scan: Mutex::new(None),
            recover: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            locked_sources: Mutex::new(vec![]),
            data_dir,
        }
    }

    pub fn files_of(&self, id: &str) -> Result<Arc<Vec<FoundFile>>, String> {
        if let Some(f) = self.cache.lock().unwrap().get(id) {
            return Ok(Arc::clone(f));
        }
        let files = Arc::new(self.store.load_files(id).map_err(|e| e.to_string())?);
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
        self.store.load_meta(id).map_err(|e| e.to_string())
    }

    pub fn new_progress() -> SharedProgress {
        Progress::new()
    }
}
