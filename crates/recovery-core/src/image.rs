//! Create a read-only sector image of a source device to another disk, so
//! scanning and recovery can run against the copy. Bad sectors are zero-filled
//! and counted; the source is never written.

use crate::device::BlockDevice;
use crate::progress::{Phase, SharedProgress};
use crate::recover::same_disk;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageReport {
    pub bytes: u64,
    pub bad_sectors: u64,
    pub path: String,
}

const BLOCK: u64 = 4 << 20;

pub fn create_image(
    dev: &dyn BlockDevice,
    source_path: &str,
    dest: &Path,
    progress: &SharedProgress,
) -> Result<ImageReport> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
        if same_disk(source_path, parent)? {
            return Err(Error::SameDisk);
        }
        let free = crate::recover::free_space(parent)?;
        if free < dev.size() {
            return Err(Error::NoSpace {
                needed: dev.size(),
                available: free,
            });
        }
    }
    let tmp = dest.with_extension("img.part");
    let mut out = std::fs::File::create(&tmp)?;
    let total = dev.size();
    progress.set_phase(Phase::Boot, total);
    progress.set_message("创建只读镜像");
    let mut buf = vec![0u8; BLOCK as usize];
    let mut off = 0u64;
    let mut bad = 0u64;
    let ss = dev.sector_size() as u64;
    while off < total {
        progress.checkpoint().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e
        })?;
        let len = BLOCK.min(total - off) as usize;
        let got = dev.read_lossy(off, &mut buf[..len]);
        if got < len {
            bad += ((len - got) as u64 + ss - 1) / ss;
            progress.add_read_error();
        }
        out.write_all(&buf[..len])?;
        off += len as u64;
        progress.add_done(len as u64);
        progress.add_bytes(len as u64);
    }
    out.sync_all()?;
    drop(out);
    std::fs::rename(&tmp, dest)?;
    Ok(ImageReport {
        bytes: total,
        bad_sectors: bad,
        path: dest.display().to_string(),
    })
}
