//! Scan orchestration: partition → volume → metadata → (optional) carving.

use crate::carve::signatures::SignatureLibrary;
use crate::carve::Carver;
use crate::device::{BlockDevice, SliceDevice};
use crate::fs::{open_volume, AllocBitmap};
use crate::model::{
    Extent, FoundFile, FsKind, Quality, ScanDepth, ScanMode, ScanRequest, Source, Summary,
};
use crate::partition::read_partitions;
use crate::progress::{Phase, SharedProgress};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOutcome {
    pub fs: FsKind,
    pub volume_label: Option<String>,
    pub volume_offset: u64,
    pub volume_len: u64,
    pub files: Vec<FoundFile>,
    pub summary: Summary,
    pub read_errors: u64,
    /// Volume parse failed; only carving was performed.
    pub carve_only: bool,
    pub warning: Option<String>,
}

/// Choose which partition to scan: explicit index, else the first with a known FS, else whole device.
pub fn select_volume<D: BlockDevice>(
    dev: &D,
    partition: Option<u32>,
) -> Result<(u64, u64, FsKind)> {
    let table = read_partitions(dev)?;
    if let Some(idx) = partition {
        let p = table
            .partitions
            .iter()
            .find(|p| p.index == idx)
            .ok_or_else(|| Error::InvalidInput(format!("分区 {} 不存在", idx)))?;
        return Ok((p.start, p.len, p.fs));
    }
    if let Some(p) = table.partitions.iter().find(|p| p.fs != FsKind::Unknown) {
        return Ok((p.start, p.len, p.fs));
    }
    Ok((0, dev.size(), FsKind::Unknown))
}

/// Run a full scan on `dev` (a disk or a volume handle).
pub fn run_scan(
    dev: Arc<dyn BlockDevice>,
    partition: Option<u32>,
    req: &ScanRequest,
    sigs: &SignatureLibrary,
    progress: &SharedProgress,
) -> Result<ScanOutcome> {
    progress.set_phase(Phase::Boot, 1);
    progress.set_message("读取分区表与引导扇区");
    let (start, len, fs_guess) = select_volume(&dev, partition)?;
    let vol_dev: Arc<dyn BlockDevice> =
        Arc::new(SliceDevice::new(Arc::clone(&dev), start, len, "volume")?);
    progress.set_done(1);

    let mut files: Vec<FoundFile> = vec![];
    let mut bitmap: Option<AllocBitmap> = None;
    let mut label = None;
    let mut fs = fs_guess;
    let mut carve_only = false;
    let mut warning = None;

    match open_volume(Arc::clone(&vol_dev)) {
        Ok(vol) => {
            fs = vol.kind();
            match vol.scan_metadata(req, progress) {
                Ok(m) => {
                    files = m.files;
                    bitmap = Some(m.bitmap);
                    label = m.volume_label;
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    warning = Some(format!("元数据扫描失败，仅进行签名雕刻: {}", e));
                    carve_only = true;
                }
            }
        }
        Err(Error::UnknownFilesystem) => {
            if req.mode == ScanMode::Directory {
                return Err(Error::Unsupported(
                    "无法识别文件系统，指定目录找回不可用；请改用全盘找回".into(),
                ));
            }
            carve_only = true;
            warning = Some("未识别到文件系统，仅进行签名雕刻".into());
        }
        Err(e) => return Err(e),
    }

    // Carving: Deep depth, or carve-only fallback in Full mode.
    let do_carve = (req.depth == ScanDepth::Deep && req.mode == ScanMode::Full)
        || (carve_only && req.mode == ScanMode::Full)
        || (req.depth == ScanDepth::Deep && req.mode == ScanMode::Directory && false);
    if do_carve {
        let align = bitmap.as_ref().map(|b| b.cluster_size).unwrap_or(512);
        let carver = Carver::new(sigs, &req.extensions, align)?;
        let ranges: Vec<Extent> = match (&bitmap, req.carve_unallocated_only) {
            (Some(b), true) => b.free_ranges(),
            _ => vec![Extent {
                offset: 0,
                len: vol_dev.size(),
            }],
        };
        let first_id = files.iter().map(|f| f.id).max().unwrap_or(0) + 1;
        let carved = carver.carve(
            Arc::clone(&vol_dev),
            &ranges,
            bitmap.as_ref(),
            req.threads,
            first_id,
            progress,
        )?;
        files.extend(carved);
    }

    // Zero probe: a deleted file whose first data sector reads as all zeros has
    // almost certainly been TRIM'd / unmapped (SSD, thin storage, VHD). Its name
    // and size survive in the metadata, the bytes do not. Report it as Low with an
    // explicit reason instead of a misleading "High", and warn when it is the norm.
    progress.set_phase(Phase::Finalize, files.len() as u64);
    progress.set_message("检查数据簇是否已被擦除");
    const PROBE: usize = 512;
    let mut buf = [0u8; PROBE];
    let mut probed = 0u64;
    let mut erased = 0u64;
    for (i, f) in files.iter_mut().enumerate() {
        if i % 512 == 0 {
            progress.checkpoint()?;
            progress.set_done(i as u64);
        }
        if f.source != Source::Metadata || f.resident.is_some() || f.size == 0 {
            continue;
        }
        let Some(e) = f.extents.first() else { continue };
        let n = PROBE.min(e.len as usize).min(f.size as usize);
        if n == 0 || e.offset + n as u64 > vol_dev.size() {
            continue;
        }
        if vol_dev.read_at(e.offset, &mut buf[..n]).is_err() {
            continue;
        }
        probed += 1;
        if buf[..n].iter().all(|b| *b == 0) {
            erased += 1;
            f.quality = Quality::Low;
            f.note = format!("数据簇内容为全零，可能已被 TRIM 擦除；{}", f.note);
        }
    }
    if probed > 0 && erased * 2 >= probed {
        let w = format!(
            "{} / {} 个已删除文件的数据簇已被擦除（SSD/TRIM 或精简置备存储）：文件名和大小仍在，内容不可恢复",
            erased, probed
        );
        warning = Some(match warning {
            Some(prev) => format!("{}；{}", prev, w),
            None => w,
        });
    }

    progress.set_message("整理结果");
    let mut summary = Summary::default();
    for f in &files {
        summary.add(f);
    }
    progress.set_done(files.len() as u64);
    progress.set_phase(Phase::Done, 1);
    Ok(ScanOutcome {
        fs,
        volume_label: label,
        volume_offset: start,
        volume_len: len,
        files,
        summary,
        read_errors: progress.snapshot().read_errors,
        carve_only,
        warning,
    })
}
