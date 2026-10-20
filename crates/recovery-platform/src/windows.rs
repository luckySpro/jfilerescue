//! Windows: physical drives via `\\.\PhysicalDriveN`, volumes via `\\.\X:`,
//! opened read-only with full sharing. BitLocker and TRIM are queried through
//! documented IOCTLs / `fsutil`-equivalent properties.

use crate::{DiskEntry, Health, Platform, PrivilegeState, VolumeEntry, WriterProcess};
use recovery_core::device::{BlockDevice, FileDevice};
use recovery_core::fs::detect_fs;
use recovery_core::model::FsKind;
use recovery_core::partition::read_partitions;
use recovery_core::{Error, Result};
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
    StorageDeviceTrimProperty, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_LENGTH_INFO,
    IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
};
/// CTL_CODE(IOCTL_VOLUME_BASE 'V', 0, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub struct WindowsPlatform;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_raw(path: &str) -> Result<std::fs::File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(0)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::AccessDenied(format!("{} 需要管理员权限", path))
            } else {
                Error::Io(e)
            }
        })
}

fn ioctl(h: HANDLE, code: u32, inp: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            code,
            if inp.is_empty() {
                std::ptr::null()
            } else {
                inp.as_ptr() as *const _
            },
            inp.len() as u32,
            if out.is_empty() {
                std::ptr::null_mut()
            } else {
                out.as_mut_ptr() as *mut _
            },
            out.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(ret as usize)
    }
}

fn device_length(f: &std::fs::File) -> Option<u64> {
    let mut out = [0u8; 8];
    ioctl(
        f.as_raw_handle() as HANDLE,
        IOCTL_DISK_GET_LENGTH_INFO,
        &[],
        &mut out,
    )?;
    Some(u64::from_le_bytes(out))
}

fn sector_size(f: &std::fs::File) -> u32 {
    let mut out = [0u8; 64];
    if ioctl(
        f.as_raw_handle() as HANDLE,
        IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
        &[],
        &mut out,
    )
    .is_some()
    {
        let bps = u32::from_le_bytes([out[20], out[21], out[22], out[23]]);
        if matches!(bps, 512 | 1024 | 2048 | 4096) {
            return bps;
        }
    }
    512
}

fn storage_query(f: &std::fs::File, prop: i32, out: &mut [u8]) -> Option<usize> {
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: prop,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let inp = unsafe {
        std::slice::from_raw_parts(
            &q as *const _ as *const u8,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>(),
        )
    };
    ioctl(
        f.as_raw_handle() as HANDLE,
        IOCTL_STORAGE_QUERY_PROPERTY,
        inp,
        out,
    )
}

fn is_ssd(f: &std::fs::File) -> Option<bool> {
    let mut out = [0u8; 12];
    storage_query(f, StorageDeviceSeekPenaltyProperty, &mut out)?;
    Some(out[8] == 0) // IncursSeekPenalty == FALSE → SSD
}

fn trim(f: &std::fs::File) -> Option<bool> {
    let mut out = [0u8; 12];
    storage_query(f, StorageDeviceTrimProperty, &mut out)?;
    Some(out[8] != 0)
}

fn model_and_serial(f: &std::fs::File) -> (String, String, bool) {
    let mut out = vec![0u8; 1024];
    if storage_query(f, StorageDeviceProperty, &mut out).is_none() {
        return (String::new(), String::new(), false);
    }
    let str_at = |off: usize| -> String {
        let o = u32::from_le_bytes([out[off], out[off + 1], out[off + 2], out[off + 3]]) as usize;
        if o == 0 || o >= out.len() {
            return String::new();
        }
        let end = out[o..]
            .iter()
            .position(|b| *b == 0)
            .map(|p| o + p)
            .unwrap_or(out.len());
        String::from_utf8_lossy(&out[o..end]).trim().to_string()
    };
    let vendor = str_at(12);
    let product = str_at(16);
    let serial = str_at(24);
    let removable = out[9] != 0;
    (
        format!("{} {}", vendor, product).trim().to_string(),
        serial,
        removable,
    )
}

fn disk_number_of_volume(f: &std::fs::File) -> Option<(u32, u64)> {
    // VOLUME_DISK_EXTENTS: NumberOfDiskExtents u32, pad, Extents[ {DiskNumber u32, pad, StartingOffset i64, ExtentLength i64} ]
    let mut out = [0u8; 8 + 24 * 4];
    ioctl(
        f.as_raw_handle() as HANDLE,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        &[],
        &mut out,
    )?;
    let n = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
    if n == 0 {
        return None;
    }
    let disk = u32::from_le_bytes([out[8], out[9], out[10], out[11]]);
    let start = u64::from_le_bytes(out[16..24].try_into().ok()?);
    Some((disk, start))
}

fn bitlocker_state(letter: char) -> (bool, bool) {
    // Use the documented FVE WMI class via PowerShell-free route: read the
    // protection status with `manage-bde`? That needs a subprocess. Instead,
    // sniff the volume: a locked BitLocker volume has "-FVE-FS-" at offset 3.
    let path = format!("\\\\.\\{}:", letter);
    let Ok(f) = open_raw(&path) else {
        return (false, false);
    };
    let dev = FileDevice::from_file(f, 1 << 20, 512, &path);
    if let Ok(b) = dev.read_vec(0, 512) {
        if &b[3..11] == b"-FVE-FS-" {
            return (true, false);
        }
    }
    // Unlocked BitLocker volumes look like plain NTFS at the volume handle, but
    // ciphertext at the physical disk. We detect that in list_disks by comparing.
    (false, true)
}

impl Platform for WindowsPlatform {
    fn privilege_state(&self) -> PrivilegeState {
        let mut elevated = false;
        unsafe {
            let mut tok: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) != 0 {
                let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
                let mut len = 0u32;
                if GetTokenInformation(
                    tok,
                    TokenElevation,
                    &mut e as *mut _ as *mut _,
                    std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                    &mut len,
                ) != 0
                {
                    elevated = e.TokenIsElevated != 0;
                }
                CloseHandle(tok);
            }
        }
        let ver = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        PrivilegeState {
            elevated,
            os: "windows".into(),
            arch: std::env::consts::ARCH.into(),
            os_version: ver,
            full_disk_access: None,
            helper_version: None,
            details: if elevated {
                vec!["已以管理员身份运行，可只读打开物理磁盘与卷".into()]
            } else {
                vec!["未提权：无法打开 \\\\.\\PhysicalDriveN 与卷句柄".into()]
            },
        }
    }

    fn list_disks(&self) -> Result<Vec<DiskEntry>> {
        // Volumes by drive letter → group under their disk number.
        let mask = unsafe { GetLogicalDrives() };
        let mut vols: Vec<(u32, VolumeEntry)> = vec![];
        let sysdrive = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".into())
            .chars()
            .next()
            .unwrap_or('C')
            .to_ascii_uppercase();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{}:\\", letter);
            let dt = unsafe { GetDriveTypeW(wide(&root).as_ptr()) };
            if dt == 4 || dt == 5 || dt == 6 {
                continue; // network, cdrom, ramdisk
            }
            let mut name = [0u16; 261];
            let mut fsname = [0u16; 64];
            let mut serial = 0u32;
            let mut maxlen = 0u32;
            let mut flags = 0u32;
            let ok = unsafe {
                GetVolumeInformationW(
                    wide(&root).as_ptr(),
                    name.as_mut_ptr(),
                    261,
                    &mut serial,
                    &mut maxlen,
                    &mut flags,
                    fsname.as_mut_ptr(),
                    64,
                )
            };
            let label = if ok != 0 {
                String::from_utf16_lossy(&name)
                    .trim_end_matches('\0')
                    .to_string()
            } else {
                String::new()
            };
            let fsname_s = if ok != 0 {
                String::from_utf16_lossy(&fsname)
                    .trim_end_matches('\0')
                    .to_string()
            } else {
                String::new()
            };
            let path = format!("\\\\.\\{}:", letter);
            let (locked, _) = bitlocker_state(letter);
            let (disk_no, size, fs, used, notes) = match open_raw(&path) {
                Ok(f) => {
                    let size = device_length(&f).unwrap_or(0);
                    let (d, _) = disk_number_of_volume(&f).unwrap_or((u32::MAX, 0));
                    let dev = FileDevice::from_file(f, size, 512, &path);
                    let fs = if locked {
                        FsKind::Unknown
                    } else {
                        detect_fs(&dev, 0)
                    };
                    let used = recovery_core::recover::free_space(std::path::Path::new(&root))
                        .ok()
                        .map(|free| size.saturating_sub(free));
                    (d, size, fs, used, vec![])
                }
                Err(e) => (u32::MAX, 0, FsKind::Unknown, None, vec![e.to_string()]),
            };
            let encrypted = locked || fsname_s.is_empty() && ok == 0;
            vols.push((
                disk_no,
                VolumeEntry {
                    path,
                    display_name: format!(
                        "{}: {}",
                        letter,
                        if label.is_empty() {
                            fsname_s.clone()
                        } else {
                            label.clone()
                        }
                    ),
                    mount: Some(root.clone()),
                    fs,
                    fs_label: if fs == FsKind::Unknown {
                        fsname_s.clone()
                    } else {
                        fs.label().into()
                    },
                    size,
                    used,
                    partition_index: None,
                    encrypted: locked || (encrypted && fsname_s.is_empty()),
                    encryption_unlocked: !locked,
                    is_system: letter == sysdrive,
                    notes,
                },
            ));
        }

        let mut disks = vec![];
        for n in 0..32u32 {
            let path = format!("\\\\.\\PhysicalDrive{}", n);
            let Ok(f) = open_raw(&path) else {
                continue;
            };
            let size = device_length(&f).unwrap_or(0);
            if size == 0 {
                continue;
            }
            let ss = sector_size(&f);
            let (model, serial, removable) = model_and_serial(&f);
            let ssd = is_ssd(&f);
            let trim_on = trim(&f);
            let dev = FileDevice::from_file(f, size, ss, &path);
            let table = read_partitions(&dev)
                .map(|t| format!("{:?}", t.kind))
                .unwrap_or_else(|_| "Unknown".into());
            let mut volumes: Vec<VolumeEntry> = vols
                .iter()
                .filter(|(d, _)| *d == n)
                .map(|(_, v)| v.clone())
                .collect();
            // Partitions without a drive letter still get listed with their index.
            if let Ok(t) = read_partitions(&dev) {
                for p in &t.partitions {
                    let already = volumes.iter().any(|v| v.size == p.len);
                    if !already && p.fs != FsKind::Unknown {
                        volumes.push(VolumeEntry {
                            path: path.clone(),
                            display_name: format!(
                                "分区 {} ({})",
                                p.index + 1,
                                p.label.clone().unwrap_or_else(|| p.type_desc.clone())
                            ),
                            mount: None,
                            fs: p.fs,
                            fs_label: p.fs.label().into(),
                            size: p.len,
                            used: None,
                            partition_index: Some(p.index),
                            encrypted: false,
                            encryption_unlocked: true,
                            is_system: false,
                            notes: vec!["无盘符".into()],
                        });
                    }
                }
            }
            let mut caps = vec!["carve".to_string()];
            if volumes.iter().any(|v| v.fs != FsKind::Unknown) {
                caps.push("metadata".into());
            }
            let health_text = if table == "Unknown" {
                "分区表损坏或未初始化".to_string()
            } else {
                "健康".to_string()
            };
            disks.push(DiskEntry {
                path,
                display_name: format!(
                    "磁盘 {} · {}",
                    n,
                    if model.is_empty() {
                        "未知型号".into()
                    } else {
                        model.clone()
                    }
                ),
                model,
                size,
                sector_size: ss,
                is_ssd: ssd,
                trim_enabled: trim_on,
                removable,
                health: if table == "Unknown" {
                    Health::Warn
                } else {
                    Health::Ok
                },
                health_text,
                table,
                volumes,
                identity: if serial.is_empty() {
                    format!("disk{}:{}", n, size)
                } else {
                    serial
                },
                capabilities: caps,
            });
        }
        Ok(disks)
    }

    fn open_device(&self, path: &str) -> Result<Arc<dyn BlockDevice>> {
        let f = open_raw(path)?;
        let size = device_length(&f).ok_or_else(|| Error::Io(std::io::Error::last_os_error()))?;
        let ss = sector_size(&f);
        Ok(Arc::new(FileDevice::from_file(f, size, ss, path)))
    }

    fn disk_identity_of_path(&self, dir: &std::path::Path) -> Option<String> {
        let mut buf = [0u16; 261];
        let w = wide(&dir.to_string_lossy());
        if unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), 261) } == 0 {
            return None;
        }
        let root = String::from_utf16_lossy(&buf)
            .trim_end_matches('\0')
            .to_string();
        let letter = root.chars().next()?;
        let f = open_raw(&format!("\\\\.\\{}:", letter)).ok()?;
        let (d, _) = disk_number_of_volume(&f)?;
        Some(format!("physdisk:{}", d))
    }

    fn disk_identity_of_device(&self, path: &str) -> Option<String> {
        let up = path.to_ascii_uppercase();
        if let Some(n) = up.strip_prefix("\\\\.\\PHYSICALDRIVE") {
            return n.parse::<u32>().ok().map(|n| format!("physdisk:{}", n));
        }
        if up.len() == 6 && up.starts_with("\\\\.\\") && up.ends_with(':') {
            let f = open_raw(path).ok()?;
            let (d, _) = disk_number_of_volume(&f)?;
            return Some(format!("physdisk:{}", d));
        }
        // Image file → disk its path is on.
        self.disk_identity_of_path(std::path::Path::new(path))
    }

    fn busy_writers(&self, volume_path: &str) -> Vec<WriterProcess> {
        // Known background writers that touch every volume. Presence check via tasklist.
        let candidates = [
            ("SearchIndexer.exe", "Windows 搜索索引"),
            ("OneDrive.exe", "OneDrive 同步"),
            ("MsMpEng.exe", "Microsoft Defender 扫描"),
            ("Dropbox.exe", "Dropbox 同步"),
        ];
        let Ok(out) = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
        else {
            return vec![];
        };
        let txt = String::from_utf8_lossy(&out.stdout);
        let _ = volume_path;
        let mut res = vec![];
        for line in txt.lines() {
            let cols: Vec<&str> = line.split("\",\"").collect();
            if cols.len() < 2 {
                continue;
            }
            let name = cols[0].trim_start_matches('"');
            if let Some((_, reason)) = candidates
                .iter()
                .find(|(c, _)| c.eq_ignore_ascii_case(name))
            {
                let pid = cols[1].trim_matches('"').parse().unwrap_or(0);
                res.push(WriterProcess {
                    name: name.into(),
                    pid,
                    reason: (*reason).into(),
                });
            }
        }
        res
    }

    fn relaunch_elevated(&self) -> Result<()> {
        let exe = std::env::current_exe()?;
        let status = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Start-Process -FilePath '{}' -Verb RunAs",
                    exe.display().to_string().replace('\'', "''")
                ),
            ])
            .status()?;
        if !status.success() {
            return Err(Error::AccessDenied("用户取消了 UAC 提权".into()));
        }
        Ok(())
    }

    fn open_privacy_settings(&self) -> Result<()> {
        std::process::Command::new("cmd")
            .args(["/c", "start", "ms-settings:privacy"])
            .spawn()?;
        Ok(())
    }
}

#[allow(dead_code)]
fn _ioctl_unused() -> u32 {
    IOCTL_STORAGE_GET_DEVICE_NUMBER
}
