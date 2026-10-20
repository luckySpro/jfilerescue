//! Write recovered files to a destination on a *different* disk.

use crate::device::BlockDevice;
use crate::model::FoundFile;
use crate::progress::{Phase, SharedProgress};
use crate::util::sanitize_name;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    Rename,
    Skip,
    Overwrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverOptions {
    pub dest: PathBuf,
    #[serde(default = "default_true")]
    pub keep_structure: bool,
    #[serde(default = "default_conflict")]
    pub conflict: Conflict,
    #[serde(default = "default_true")]
    pub verify: bool,
    #[serde(default = "default_true")]
    pub restore_times: bool,
}

fn default_true() -> bool {
    true
}
fn default_conflict() -> Conflict {
    Conflict::Rename
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Ok,
    OkWithWarning,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    pub id: u64,
    pub name: String,
    pub path: Option<String>,
    pub status: ItemStatus,
    pub message: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecoverReport {
    pub items: Vec<ItemResult>,
    pub ok: u64,
    pub warned: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes: u64,
}

/// True when `dest_dir` lives on the same physical disk/volume as `source`.
/// `source` is a platform device path (`\\.\PhysicalDrive1`, `\\.\D:`, `/dev/rdisk4`).
/// Image *files* are never "same disk": their blocks are already allocated, so
/// writing elsewhere on that disk cannot overwrite the data being recovered.
/// The platform crate refines device paths with real volume → disk mapping.
pub fn same_disk(source: &str, dest_dir: &Path) -> Result<bool> {
    if Path::new(source).is_file() {
        return Ok(false);
    }
    let src = source.to_ascii_lowercase();
    let dest = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());
    let dest_s = dest
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace("\\\\?\\", "");
    #[cfg(windows)]
    {
        // Volume handle `\\.\D:` vs dest `d:\…`.
        if src.len() == 6 && src.starts_with("\\\\.\\") && src.ends_with(':') {
            let letter = src.as_bytes()[4] as char;
            if letter.is_ascii_alphabetic() && dest_s.starts_with(&format!("{}:", letter)) {
                return Ok(true);
            }
        }
    }
    #[cfg(unix)]
    {
        // /dev/rdisk4s2 vs a destination mounted from that device.
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if let (Ok(a), Ok(b)) = (std::fs::metadata(source), std::fs::metadata(&dest)) {
            if (a.file_type().is_block_device() || a.file_type().is_char_device())
                && a.rdev() == b.dev()
            {
                return Ok(true);
            }
        }
    }
    let _ = (&src, &dest_s);
    Ok(false)
}

/// Free bytes available at `dir` (portable approximation via platform APIs in the platform crate; here via `fs2`-free fallback).
pub fn free_space(dir: &Path) -> Result<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        extern "system" {
            fn GetDiskFreeSpaceExW(
                dir: *const u16,
                free: *mut u64,
                total: *mut u64,
                total_free: *mut u64,
            ) -> i32;
        }
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
        if ok == 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(free)
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| Error::InvalidInput("路径包含 NUL".into()))?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        // Field widths differ per platform (u32/u64); widen explicitly.
        #[allow(clippy::unnecessary_cast)]
        Ok((st.f_bavail as u128 * st.f_frsize as u128) as u64)
    }
}

fn unique_path(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10_000 {
        let c = p.with_file_name(format!("{} ({}){}", stem, i, ext));
        if !c.exists() {
            return c;
        }
    }
    p.with_file_name(format!("{}-{}{}", stem, std::process::id(), ext))
}

/// Read the file data in order, calling `sink` for each chunk.
pub fn read_file_data(
    dev: &dyn BlockDevice,
    f: &FoundFile,
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    if let Some(r) = &f.resident {
        sink(r)?;
        return Ok(r.len() as u64);
    }
    let mut remaining = f.size;
    let mut buf = vec![0u8; 4 << 20];
    let mut lost = 0u64;
    for e in &f.extents {
        let mut off = e.offset;
        let mut left = e.len.min(remaining);
        while left > 0 {
            let n = (buf.len() as u64).min(left) as usize;
            let got = dev.read_lossy(off, &mut buf[..n]);
            if got < n {
                lost += (n - got) as u64;
            }
            sink(&buf[..n])?;
            off += n as u64;
            left -= n as u64;
            remaining -= n as u64;
        }
    }
    if lost > 0 {
        return Err(Error::Io(std::io::Error::other(format!(
            "{} 字节读取失败，已用零填充",
            lost
        ))));
    }
    Ok(f.size - remaining)
}

/// Peek up to `n` bytes for previews.
pub fn peek(dev: &dyn BlockDevice, f: &FoundFile, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    let _ = read_file_data(dev, f, |c| {
        if out.len() < n {
            let take = (n - out.len()).min(c.len());
            out.extend_from_slice(&c[..take]);
        }
        Ok(())
    });
    out.truncate(n);
    out
}

/// Light-weight structural verification by extension.
pub fn verify(ext: &str, head: &[u8], tail: &[u8]) -> Option<String> {
    match ext {
        "jpg" | "jpeg" => {
            if !head.starts_with(&[0xFF, 0xD8]) {
                return Some("文件头不是 JPEG".into());
            }
            if !tail.ends_with(&[0xFF, 0xD9]) {
                return Some("尾部缺少 EOI 标记".into());
            }
        }
        "png" => {
            if !head.starts_with(b"\x89PNG") {
                return Some("文件头不是 PNG".into());
            }
            if !tail.windows(4).any(|w| w == b"IEND") {
                return Some("尾部缺少 IEND".into());
            }
        }
        "pdf" => {
            if !head.starts_with(b"%PDF") {
                return Some("文件头不是 PDF".into());
            }
            if !tail.windows(5).any(|w| w == b"%%EOF") {
                return Some("尾部缺少 %%EOF".into());
            }
        }
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" => {
            if !head.starts_with(b"PK\x03\x04") {
                return Some("文件头不是 ZIP".into());
            }
            if !tail.windows(4).any(|w| w == b"PK\x05\x06") {
                return Some("尾部缺少 ZIP 中央目录结束记录".into());
            }
        }
        "gif" if !tail.ends_with(&[0x3B]) => {
            return Some("尾部缺少 GIF 结束符".into());
        }
        _ => {}
    }
    None
}

pub fn recover(
    dev: &dyn BlockDevice,
    source_path: &str,
    files: &[FoundFile],
    opts: &RecoverOptions,
    progress: &SharedProgress,
) -> Result<RecoverReport> {
    std::fs::create_dir_all(&opts.dest)?;
    if same_disk(source_path, &opts.dest)? {
        return Err(Error::SameDisk);
    }
    let needed: u64 = files.iter().map(|f| f.size).sum();
    let free = free_space(&opts.dest)?;
    if free < needed {
        return Err(Error::NoSpace {
            needed,
            available: free,
        });
    }
    progress.set_phase(Phase::Finalize, needed.max(1));
    progress.set_message("恢复文件");
    let mut report = RecoverReport::default();
    for f in files {
        progress.checkpoint()?;
        let res = recover_one(dev, f, opts, progress);
        match res.status {
            ItemStatus::Ok => report.ok += 1,
            ItemStatus::OkWithWarning => report.warned += 1,
            ItemStatus::Skipped => report.skipped += 1,
            ItemStatus::Failed => report.failed += 1,
        }
        report.bytes += res.bytes;
        report.items.push(res);
    }
    Ok(report)
}

fn recover_one(
    dev: &dyn BlockDevice,
    f: &FoundFile,
    opts: &RecoverOptions,
    progress: &SharedProgress,
) -> ItemResult {
    let rel_dir: PathBuf = if opts.keep_structure {
        match &f.dir {
            Some(d) if !d.is_empty() => d
                .split('/')
                .filter(|s| !s.is_empty())
                .map(sanitize_name)
                .collect(),
            Some(_) => PathBuf::new(),
            None => PathBuf::from("_carved").join(if f.ext.is_empty() {
                "unknown".to_string()
            } else {
                f.ext.clone()
            }),
        }
    } else {
        PathBuf::new()
    };
    let dir = opts.dest.join(&rel_dir);
    let fname = sanitize_name(&f.name);
    let mut target = dir.join(&fname);
    let fail = |m: String| ItemResult {
        id: f.id,
        name: f.name.clone(),
        path: None,
        status: ItemStatus::Failed,
        message: m,
        bytes: 0,
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(format!("创建目录失败: {}", e));
    }
    if target.exists() {
        match opts.conflict {
            Conflict::Skip => {
                return ItemResult {
                    id: f.id,
                    name: f.name.clone(),
                    path: Some(target.display().to_string()),
                    status: ItemStatus::Skipped,
                    message: "目标已存在".into(),
                    bytes: 0,
                }
            }
            Conflict::Rename => target = unique_path(&target),
            Conflict::Overwrite => {}
        }
    }
    let tmp = target.with_extension(format!(
        "{}.jfr-part",
        target
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    let mut out = match std::fs::File::create(&tmp) {
        Ok(o) => o,
        Err(e) => return fail(format!("创建文件失败: {}", e)),
    };
    let mut head = Vec::with_capacity(16);
    let mut tail: Vec<u8> = Vec::with_capacity(64);
    let mut written = 0u64;
    let mut io_warn: Option<String> = None;
    let r = read_file_data(dev, f, |chunk| {
        if head.len() < 16 {
            let take = (16 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        if chunk.len() >= 64 {
            tail.clear();
            tail.extend_from_slice(&chunk[chunk.len() - 64..]);
        } else {
            tail.extend_from_slice(chunk);
            if tail.len() > 64 {
                let cut = tail.len() - 64;
                tail.drain(..cut);
            }
        }
        out.write_all(chunk)?;
        written += chunk.len() as u64;
        progress.add_done(chunk.len() as u64);
        Ok(())
    });
    match r {
        Ok(_) => {}
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::Other => io_warn = Some(e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return fail(format!("读取失败: {}", e));
        }
    }
    if let Err(e) = out.sync_all() {
        let _ = std::fs::remove_file(&tmp);
        return fail(format!("写入失败: {}", e));
    }
    drop(out);
    if opts.conflict == Conflict::Overwrite && target.exists() {
        let _ = std::fs::remove_file(&target);
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return fail(format!("重命名失败: {}", e));
    }
    if opts.restore_times {
        let mtime = f
            .modified_at
            .map(|t| filetime::FileTime::from_unix_time(t, 0));
        if let Some(m) = mtime {
            let _ = filetime::set_file_mtime(&target, m);
        }
    }
    let mut warn = io_warn;
    if opts.verify && warn.is_none() {
        warn = verify(&f.ext, &head, &tail);
    }
    ItemResult {
        id: f.id,
        name: f.name.clone(),
        path: Some(target.display().to_string()),
        status: if warn.is_some() {
            ItemStatus::OkWithWarning
        } else {
            ItemStatus::Ok
        },
        message: warn.unwrap_or_else(|| "校验通过".into()),
        bytes: written,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;
    use crate::model::{Extent, Quality, Source};

    #[test]
    fn recover_from_mem_device() {
        let mut data = vec![0u8; 8192];
        data[1024..1024 + 5].copy_from_slice(b"hello");
        let dev = MemDevice::new(data);
        let f = FoundFile {
            id: 1,
            name: "a.txt".into(),
            dir: Some("docs/sub".into()),
            size: 5,
            source: Source::Metadata,
            quality: Quality::High,
            overwritten_pct: 0,
            deleted_at: None,
            modified_at: Some(1_700_000_000),
            created_at: None,
            ext: "txt".into(),
            extents: vec![Extent {
                offset: 1024,
                len: 4096,
            }],
            resident: None,
            name_damaged: false,
            note: String::new(),
        };
        let tmp = tempfile::tempdir().unwrap();
        let opts = RecoverOptions {
            dest: tmp.path().join("out"),
            keep_structure: true,
            conflict: Conflict::Rename,
            verify: true,
            restore_times: true,
        };
        let p = crate::progress::Progress::new();
        // source_path = a nonexistent image path on a different root to pass same_disk heuristics
        let rep = recover(&dev, "mem://test", &[f.clone(), f.clone()], &opts, &p).unwrap();
        assert_eq!(rep.ok, 2);
        let a = tmp.path().join("out/docs/sub/a.txt");
        assert_eq!(std::fs::read(&a).unwrap(), b"hello");
        assert!(tmp.path().join("out/docs/sub/a (1).txt").exists());
    }
}
