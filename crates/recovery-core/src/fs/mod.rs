//! File system parsers. Each parser is read-only and produces [`FoundFile`]s
//! for deleted entries plus an allocation bitmap for the carver.

pub mod apfs;
pub mod fat;
pub mod ntfs;

use crate::device::BlockDevice;
use crate::model::{FoundFile, FsKind, ScanRequest};
use crate::progress::SharedProgress;
use crate::Result;

/// Allocation state of a volume, cluster granular. `1` = allocated.
/// Allocation state of a volume, cluster granular. `1` = allocated.
///
/// Bit `i` describes the cluster whose first byte is `base + i * cluster_size`.
/// NTFS/APFS number clusters from the volume start (`base = 0`); FAT/exFAT
/// number them from the data area starting at cluster 2 (`base = data_offset`).
#[derive(Debug, Clone)]
pub struct AllocBitmap {
    pub cluster_size: u64,
    pub clusters: u64,
    pub base: u64,
    bits: Vec<u8>,
}

impl AllocBitmap {
    pub fn new(cluster_size: u64, clusters: u64) -> Self {
        Self::new_at(cluster_size, clusters, 0)
    }
    pub fn new_at(cluster_size: u64, clusters: u64, base: u64) -> Self {
        Self {
            cluster_size,
            clusters,
            base,
            bits: vec![0u8; clusters.div_ceil(8) as usize],
        }
    }
    pub fn all_allocated(cluster_size: u64, clusters: u64) -> Self {
        Self::all_allocated_at(cluster_size, clusters, 0)
    }
    pub fn all_allocated_at(cluster_size: u64, clusters: u64, base: u64) -> Self {
        Self {
            cluster_size,
            clusters,
            base,
            bits: vec![0xFF; clusters.div_ceil(8) as usize],
        }
    }
    pub fn from_raw(cluster_size: u64, clusters: u64, raw: Vec<u8>) -> Self {
        Self::from_raw_at(cluster_size, clusters, raw, 0)
    }
    pub fn from_raw_at(cluster_size: u64, clusters: u64, raw: Vec<u8>, base: u64) -> Self {
        let mut bits = raw;
        bits.resize(clusters.div_ceil(8) as usize, 0xFF);
        Self {
            cluster_size,
            clusters,
            base,
            bits,
        }
    }
    /// Cluster index for a byte offset; `None` when the offset lies before `base`.
    fn index_of(&self, offset: u64) -> Option<u64> {
        offset.checked_sub(self.base).map(|o| o / self.cluster_size)
    }
    pub fn set(&mut self, c: u64, allocated: bool) {
        if c >= self.clusters {
            return;
        }
        let i = (c / 8) as usize;
        let m = 1u8 << (c % 8);
        if allocated {
            self.bits[i] |= m;
        } else {
            self.bits[i] &= !m;
        }
    }
    pub fn is_allocated(&self, c: u64) -> bool {
        if c >= self.clusters {
            return true;
        }
        self.bits[(c / 8) as usize] & (1u8 << (c % 8)) != 0
    }
    pub fn free_count(&self) -> u64 {
        (0..self.clusters)
            .filter(|c| !self.is_allocated(*c))
            .count() as u64
    }
    /// Fraction (0..=100) of the given byte ranges currently allocated.
    pub fn overwritten_pct(&self, extents: &[crate::model::Extent]) -> u8 {
        let mut total = 0u64;
        let mut alloc = 0u64;
        for e in extents {
            let end = e.offset + e.len.max(1) - 1;
            let (Some(first), Some(last)) = (self.index_of(e.offset), self.index_of(end)) else {
                // Before the data area (FAT/boot region): never a data cluster → count as allocated.
                total += 1;
                alloc += 1;
                continue;
            };
            for c in first..=last {
                total += 1;
                if self.is_allocated(c) {
                    alloc += 1;
                }
            }
        }
        (alloc * 100).checked_div(total).unwrap_or(0) as u8
    }
    /// Iterate contiguous free byte ranges (for carving).
    pub fn free_ranges(&self) -> Vec<crate::model::Extent> {
        let mut out = vec![];
        let mut start: Option<u64> = None;
        for c in 0..=self.clusters {
            let free = c < self.clusters && !self.is_allocated(c);
            match (start, free) {
                (None, true) => start = Some(c),
                (Some(s), false) => {
                    out.push(crate::model::Extent {
                        offset: self.base + s * self.cluster_size,
                        len: (c - s) * self.cluster_size,
                    });
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
}

/// Output of a metadata scan.
pub struct MetadataScan {
    pub files: Vec<FoundFile>,
    pub bitmap: AllocBitmap,
    pub volume_label: Option<String>,
}

/// A mounted (parsed) read-only volume.
pub trait Volume: Send + Sync {
    fn kind(&self) -> FsKind;
    fn label(&self) -> Option<String>;
    fn cluster_size(&self) -> u64;
    fn total_clusters(&self) -> u64;
    /// Enumerate deleted entries matching `req` and build the allocation bitmap.
    fn scan_metadata(&self, req: &ScanRequest, progress: &SharedProgress) -> Result<MetadataScan>;
}

/// Sniff a file system at `offset` without fully parsing it.
pub fn detect_fs<D: BlockDevice>(dev: &D, offset: u64) -> FsKind {
    let Ok(b) = dev.read_vec(offset, 512) else {
        return FsKind::Unknown;
    };
    if &b[3..11] == b"NTFS    " {
        return FsKind::Ntfs;
    }
    if &b[3..11] == b"EXFAT   " {
        return FsKind::ExFat;
    }
    if &b[82..90] == b"FAT32   " {
        return FsKind::Fat32;
    }
    if &b[54..62] == b"FAT16   " {
        return FsKind::Fat16;
    }
    if &b[54..62] == b"FAT12   " {
        return FsKind::Fat12;
    }
    // Generic FAT heuristics: valid jump, bytes/sector, sectors/cluster.
    if (b[0] == 0xEB || b[0] == 0xE9) && b[510] == 0x55 && b[511] == 0xAA {
        let bps = crate::util::le_u16(&b, 11);
        let spc = b[13];
        if matches!(bps, 512 | 1024 | 2048 | 4096) && spc.is_power_of_two() && b[16] >= 1 {
            return fat::classify_fat(&b);
        }
    }
    // APFS container superblock magic "NXSB" at 0x20.
    if b.len() >= 36 && &b[32..36] == b"NXSB" {
        return FsKind::Apfs;
    }
    FsKind::Unknown
}

/// Open a volume at `dev` (which must already be sliced to the partition).
pub fn open_volume(dev: std::sync::Arc<dyn BlockDevice>) -> Result<Box<dyn Volume>> {
    match detect_fs(&dev, 0) {
        FsKind::Ntfs => Ok(Box::new(ntfs::NtfsVolume::open(dev)?)),
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => Ok(Box::new(fat::FatVolume::open(dev)?)),
        FsKind::ExFat => Ok(Box::new(fat::ExFatVolume::open(dev)?)),
        FsKind::Apfs => Ok(Box::new(apfs::ApfsVolume::open(dev)?)),
        FsKind::Unknown => Err(crate::Error::UnknownFilesystem),
    }
}
