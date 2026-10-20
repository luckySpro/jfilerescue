//! macOS: disks via `diskutil list -plist`, raw access through `/dev/rdiskN`.
//! Opening raw devices needs root; the GUI uses a privileged helper (see
//! `helper/`) that hands back a read-only file descriptor. The CLI can run
//! under sudo directly.

use crate::{DiskEntry, Health, Platform, PrivilegeState, VolumeEntry, WriterProcess};
use recovery_core::device::{BlockDevice, FileDevice};
use recovery_core::fs::detect_fs;
use recovery_core::model::FsKind;
use recovery_core::{Error, Result};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::Arc;

pub struct MacPlatform;

fn diskutil_plist(args: &[&str]) -> Option<plist::Value> {
    let out = std::process::Command::new("/usr/sbin/diskutil")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    plist::from_bytes(&out.stdout).ok()
}

fn dict_str(d: &plist::Dictionary, k: &str) -> String {
    d.get(k)
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string()
}
fn dict_u64(d: &plist::Dictionary, k: &str) -> u64 {
    d.get(k)
        .and_then(|v| {
            v.as_unsigned_integer()
                .or_else(|| v.as_signed_integer().map(|i| i as u64))
        })
        .unwrap_or(0)
}
fn dict_bool(d: &plist::Dictionary, k: &str) -> Option<bool> {
    d.get(k).and_then(|v| v.as_boolean())
}

fn device_size(f: &std::fs::File) -> Option<(u64, u32)> {
    // DKIOCGETBLOCKSIZE / DKIOCGETBLOCKCOUNT
    const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
    const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
    let mut bs: u32 = 0;
    let mut bc: u64 = 0;
    unsafe {
        if libc::ioctl(f.as_raw_fd(), DKIOCGETBLOCKSIZE, &mut bs) != 0 {
            return None;
        }
        if libc::ioctl(f.as_raw_fd(), DKIOCGETBLOCKCOUNT, &mut bc) != 0 {
            return None;
        }
    }
    Some((bs as u64 * bc, bs))
}

pub fn open_raw(path: &str) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::AccessDenied(format!("{} 需要管理员权限（或完全磁盘访问）", path))
            } else {
                Error::Io(e)
            }
        })
}

/// Wrap an already-open fd (from the privileged helper).
pub fn device_from_fd(fd: i32, path: &str) -> Result<Arc<dyn BlockDevice>> {
    use std::os::unix::io::FromRawFd;
    let f = unsafe { std::fs::File::from_raw_fd(fd) };
    let (size, bs) = device_size(&f)
        .or_else(|| f.metadata().ok().map(|m| (m.len(), 512)))
        .ok_or_else(|| Error::Io(std::io::Error::last_os_error()))?;
    Ok(Arc::new(FileDevice::from_file(f, size, bs, path)))
}

fn mount_point_device(dir: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let dev = unsafe { std::ffi::CStr::from_ptr(st.f_mntfromname.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Some(dev)
}

fn parent_disk(dev: &str) -> String {
    // /dev/disk4s2 → disk4 ; APFS synthesized disk3s5 → physical store via diskutil info.
    let name = dev.trim_start_matches("/dev/").trim_start_matches('r');
    let base: String = {
        let mut s = String::new();
        let mut chars = name.chars().peekable();
        while let Some(c) = chars.next() {
            if c == 's'
                && chars.peek().map(|n| n.is_ascii_digit()).unwrap_or(false)
                && s.starts_with("disk")
                && s.len() > 4
            {
                break;
            }
            s.push(c);
        }
        s
    };
    if let Some(plist::Value::Dictionary(info)) = diskutil_plist(&["info", "-plist", &base]) {
        if let Some(stores) = info.get("APFSPhysicalStores").and_then(|v| v.as_array()) {
            if let Some(first) = stores.first().and_then(|v| v.as_dictionary()) {
                let pd = dict_str(first, "DeviceIdentifier");
                if !pd.is_empty() {
                    return parent_disk(&pd);
                }
            }
        }
    }
    base
}

impl Platform for MacPlatform {
    fn privilege_state(&self) -> PrivilegeState {
        let elevated = unsafe { libc::geteuid() } == 0;
        let ver = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        // FDA probe: reading a TCC-protected path succeeds only with Full Disk Access.
        let home = std::env::var("HOME").unwrap_or_default();
        let fda = std::fs::read_dir(format!("{}/Library/Mail", home))
            .map(|_| true)
            .or_else(|_| {
                std::fs::metadata(format!("{}/Library/Safari/Bookmarks.plist", home)).map(|_| true)
            })
            .ok();
        let helper_version = std::fs::read_to_string(
            "/Library/PrivilegedHelperTools/com.mjgroup.jfilerescue.helper.version",
        )
        .ok()
        .map(|s| s.trim().to_string());
        PrivilegeState {
            elevated,
            os: "macos".into(),
            arch: std::env::consts::ARCH.into(),
            os_version: ver,
            full_disk_access: fda,
            helper_version,
            details: vec![if elevated {
                "以 root 运行".into()
            } else {
                "GUI 以普通用户运行；原始设备通过特权助手只读打开".into()
            }],
        }
    }

    fn list_disks(&self) -> Result<Vec<DiskEntry>> {
        let Some(plist::Value::Dictionary(list)) = diskutil_plist(&["list", "-plist", "physical"])
        else {
            return Err(Error::Other("diskutil 不可用".into()));
        };
        let mut disks = vec![];
        let whole: Vec<String> = list
            .get("WholeDisks")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_string().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for d in whole {
            let Some(plist::Value::Dictionary(info)) = diskutil_plist(&["info", "-plist", &d])
            else {
                continue;
            };
            let size = dict_u64(&info, "TotalSize").max(dict_u64(&info, "Size"));
            let ssd = dict_bool(&info, "SolidState");
            let model = dict_str(&info, "MediaName");
            let removable = dict_bool(&info, "RemovableMedia")
                .or(dict_bool(&info, "Removable"))
                .unwrap_or(false);
            let internal = dict_bool(&info, "Internal").unwrap_or(false);
            let bs = dict_u64(&info, "DeviceBlockSize").max(512) as u32;
            let table = dict_str(&info, "Content");
            let smart = dict_str(&info, "SMARTStatus");

            // Volumes under this disk, including APFS volumes on its container.
            let mut volumes = vec![];
            let Some(plist::Value::Dictionary(full)) = diskutil_plist(&["list", "-plist", &d])
            else {
                continue;
            };
            let mut idents: Vec<String> = vec![];
            if let Some(all) = full.get("AllDisksAndPartitions").and_then(|v| v.as_array()) {
                for e in all.iter().filter_map(|v| v.as_dictionary()) {
                    for p in e
                        .get("Partitions")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_dictionary())
                    {
                        idents.push(dict_str(p, "DeviceIdentifier"));
                    }
                    for p in e
                        .get("APFSVolumes")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_dictionary())
                    {
                        idents.push(dict_str(p, "DeviceIdentifier"));
                    }
                }
            }
            // APFS containers synthesized from this disk.
            if let Some(plist::Value::Dictionary(apfs)) =
                diskutil_plist(&["apfs", "list", "-plist"])
            {
                for c in apfs
                    .get("Containers")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_dictionary())
                {
                    let on_this = c
                        .get("PhysicalStores")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_dictionary())
                                .any(|s| dict_str(s, "DeviceIdentifier").starts_with(&d))
                        })
                        .unwrap_or(false);
                    if !on_this {
                        continue;
                    }
                    for v in c
                        .get("Volumes")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_dictionary())
                    {
                        idents.push(dict_str(v, "DeviceIdentifier"));
                    }
                }
            }
            idents.sort();
            idents.dedup();
            for id in idents {
                let Some(plist::Value::Dictionary(vi)) = diskutil_plist(&["info", "-plist", &id])
                else {
                    continue;
                };
                let fs_pers = dict_str(&vi, "FilesystemType");
                let fs = match fs_pers.as_str() {
                    "apfs" => FsKind::Apfs,
                    "ntfs" => FsKind::Ntfs,
                    "exfat" => FsKind::ExFat,
                    "msdos" => FsKind::Fat32,
                    _ => FsKind::Unknown,
                };
                if fs == FsKind::Unknown && dict_str(&vi, "Content") != "Microsoft Basic Data" {
                    if !fs_pers.is_empty() {
                        // hfs etc. — listed but unsupported
                    } else {
                        continue;
                    }
                }
                let vsize = dict_u64(&vi, "TotalSize").max(dict_u64(&vi, "Size"));
                let free = dict_u64(&vi, "APFSContainerFree").max(dict_u64(&vi, "FreeSpace"));
                let encrypted = dict_bool(&vi, "Encrypted")
                    .or(dict_bool(&vi, "FileVault"))
                    .unwrap_or(false);
                let locked = dict_bool(&vi, "Locked").unwrap_or(false);
                let mount = dict_str(&vi, "MountPoint");
                let name = dict_str(&vi, "VolumeName");
                let role = dict_str(&vi, "APFSVolumeRole");
                let mut notes = vec![];
                if encrypted {
                    notes.push("FileVault/加密卷：设备层为密文，签名雕刻不可用".into());
                }
                if fs == FsKind::Unknown {
                    notes.push(format!(
                        "不支持的文件系统: {}",
                        if fs_pers.is_empty() {
                            "未知".into()
                        } else {
                            fs_pers.clone()
                        }
                    ));
                }
                volumes.push(VolumeEntry {
                    path: format!("/dev/r{}", id),
                    display_name: format!(
                        "{} ({})",
                        if name.is_empty() { id.clone() } else { name },
                        id
                    ),
                    mount: if mount.is_empty() {
                        None
                    } else {
                        Some(mount.clone())
                    },
                    fs,
                    fs_label: if fs == FsKind::Unknown {
                        fs_pers.to_uppercase()
                    } else {
                        fs.label().into()
                    },
                    size: vsize,
                    used: if free > 0 {
                        Some(vsize.saturating_sub(free))
                    } else {
                        None
                    },
                    partition_index: None,
                    encrypted,
                    encryption_unlocked: !locked,
                    is_system: mount == "/"
                        || role.contains("System")
                        || role.contains("Data") && internal,
                    notes,
                });
            }
            let mut caps = vec!["carve".to_string()];
            if volumes.iter().any(|v| v.fs != FsKind::Unknown) {
                caps.push("metadata".into());
            }
            disks.push(DiskEntry {
                path: format!("/dev/r{}", d),
                display_name: format!(
                    "{} ({})",
                    if model.is_empty() {
                        d.clone()
                    } else {
                        model.clone()
                    },
                    d
                ),
                model,
                size,
                sector_size: bs,
                is_ssd: ssd,
                trim_enabled: if ssd == Some(true) && internal {
                    Some(true)
                } else {
                    None
                },
                removable,
                health: match smart.as_str() {
                    "Verified" | "Not Supported" | "" => Health::Ok,
                    _ => Health::Bad,
                },
                health_text: if smart.is_empty() || smart == "Not Supported" {
                    "健康".into()
                } else {
                    smart
                },
                table,
                volumes,
                identity: {
                    let u = dict_str(&info, "DiskUUID");
                    if u.is_empty() {
                        format!("{}:{}", d, size)
                    } else {
                        u
                    }
                },
                capabilities: caps,
            });
        }
        Ok(disks)
    }

    fn open_device(&self, path: &str) -> Result<Arc<dyn BlockDevice>> {
        let f = open_raw(path)?;
        let (size, bs) =
            device_size(&f).ok_or_else(|| Error::Io(std::io::Error::last_os_error()))?;
        Ok(Arc::new(FileDevice::from_file(f, size, bs, path)))
    }

    fn disk_identity_of_path(&self, dir: &Path) -> Option<String> {
        let dev = mount_point_device(dir)?;
        Some(format!("disk:{}", parent_disk(&dev)))
    }

    fn disk_identity_of_device(&self, path: &str) -> Option<String> {
        if path.starts_with("/dev/") {
            return Some(format!("disk:{}", parent_disk(path)));
        }
        // Image files are regular files: never "same disk".
        None
    }

    fn busy_writers(&self, volume_path: &str) -> Vec<WriterProcess> {
        let candidates = [
            ("mds_stores", "Spotlight 索引"),
            ("mdworker", "Spotlight 索引"),
            ("backupd", "Time Machine 备份"),
            ("bird", "iCloud Drive 同步"),
            ("photoanalysisd", "照片分析"),
        ];
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axo", "pid=,comm="])
            .output()
        else {
            return vec![];
        };
        let _ = volume_path;
        let txt = String::from_utf8_lossy(&out.stdout);
        let mut res = vec![];
        for line in txt.lines() {
            let mut it = line.trim().splitn(2, ' ');
            let pid: u32 = it.next().and_then(|p| p.trim().parse().ok()).unwrap_or(0);
            let comm = it.next().unwrap_or("").trim();
            let base = comm.rsplit('/').next().unwrap_or(comm);
            if let Some((_, reason)) = candidates.iter().find(|(c, _)| base.starts_with(c)) {
                res.push(WriterProcess {
                    name: base.into(),
                    pid,
                    reason: (*reason).into(),
                });
            }
        }
        res
    }

    fn relaunch_elevated(&self) -> Result<()> {
        Err(Error::Unsupported(
            "macOS 通过特权助手授权，不整体提权 GUI".into(),
        ))
    }

    fn open_privacy_settings(&self) -> Result<()> {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn()?;
        Ok(())
    }
}

#[allow(dead_code)]
fn _detect(dev: &FileDevice) -> FsKind {
    detect_fs(dev, 0)
}
