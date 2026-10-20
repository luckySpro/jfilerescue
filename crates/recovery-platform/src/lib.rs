//! Platform layer: enumerate disks/volumes, open them read-only, and report
//! facts the UI needs (TRIM, BitLocker/FileVault, privilege state, busy writers).

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

use recovery_core::device::BlockDevice;
use recovery_core::model::FsKind;
use recovery_core::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ok,
    Warn,
    Bad,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeEntry {
    /// Path usable with `open_device` (volume handle or partition device).
    pub path: String,
    pub display_name: String,
    pub mount: Option<String>,
    pub fs: FsKind,
    pub fs_label: String,
    pub size: u64,
    pub used: Option<u64>,
    pub partition_index: Option<u32>,
    pub encrypted: bool,
    pub encryption_unlocked: bool,
    pub is_system: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskEntry {
    pub path: String,
    pub display_name: String,
    pub model: String,
    pub size: u64,
    pub sector_size: u32,
    pub is_ssd: Option<bool>,
    pub trim_enabled: Option<bool>,
    pub removable: bool,
    pub health: Health,
    pub health_text: String,
    pub table: String,
    pub volumes: Vec<VolumeEntry>,
    pub identity: String,
    /// Which modes are available at this level.
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivilegeState {
    pub elevated: bool,
    pub os: String,
    pub arch: String,
    pub os_version: String,
    /// macOS: Full Disk Access granted (None = unknown).
    pub full_disk_access: Option<bool>,
    pub helper_version: Option<String>,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterProcess {
    pub name: String,
    pub pid: u32,
    pub reason: String,
}

pub trait Platform {
    fn privilege_state(&self) -> PrivilegeState;
    fn list_disks(&self) -> Result<Vec<DiskEntry>>;
    fn open_device(&self, path: &str) -> Result<Arc<dyn BlockDevice>>;
    /// Maps a destination directory to the disk identity it lives on, for same-disk refusal.
    fn disk_identity_of_path(&self, dir: &std::path::Path) -> Option<String>;
    /// Identity of a source device path (volume or disk).
    fn disk_identity_of_device(&self, path: &str) -> Option<String>;
    fn busy_writers(&self, volume_path: &str) -> Vec<WriterProcess>;
    fn relaunch_elevated(&self) -> Result<()>;
    fn open_privacy_settings(&self) -> Result<()>;
}

pub fn current() -> Box<dyn Platform + Send + Sync> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsPlatform)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPlatform)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(Unsupported)
    }
}

/// Same-disk check that understands real disks (volume → parent disk).
pub fn same_disk(p: &dyn Platform, source_path: &str, dest: &std::path::Path) -> bool {
    if recovery_core::recover::same_disk(source_path, dest).unwrap_or(false) {
        return true;
    }
    match (
        p.disk_identity_of_device(source_path),
        p.disk_identity_of_path(dest),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub struct Unsupported;

#[cfg(not(any(windows, target_os = "macos")))]
impl Platform for Unsupported {
    fn privilege_state(&self) -> PrivilegeState {
        PrivilegeState {
            elevated: false,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            os_version: String::new(),
            full_disk_access: None,
            helper_version: None,
            details: vec!["当前平台不支持设备枚举，仅能扫描镜像文件".into()],
        }
    }
    fn list_disks(&self) -> Result<Vec<DiskEntry>> {
        Ok(vec![])
    }
    fn open_device(&self, path: &str) -> Result<Arc<dyn BlockDevice>> {
        Ok(Arc::new(recovery_core::device::FileDevice::open(
            std::path::Path::new(path),
        )?))
    }
    fn disk_identity_of_path(&self, _dir: &std::path::Path) -> Option<String> {
        None
    }
    fn disk_identity_of_device(&self, _path: &str) -> Option<String> {
        None
    }
    fn busy_writers(&self, _v: &str) -> Vec<WriterProcess> {
        vec![]
    }
    fn relaunch_elevated(&self) -> Result<()> {
        Err(recovery_core::Error::Unsupported("不支持".into()))
    }
    fn open_privacy_settings(&self) -> Result<()> {
        Err(recovery_core::Error::Unsupported("不支持".into()))
    }
}

/// Open either a real device (via platform) or an image file.
pub fn open_source(p: &dyn Platform, path: &str) -> Result<Arc<dyn BlockDevice>> {
    let pb = std::path::Path::new(path);
    if pb.is_file() {
        return Ok(Arc::new(recovery_core::device::FileDevice::open(pb)?));
    }
    p.open_device(path)
}
