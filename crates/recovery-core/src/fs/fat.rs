//! Read-only FAT12/16/32 and exFAT parsers for deleted directory entries.
//!
//! FAT: deleted entries are marked `0xE5` and keep size + first cluster. The
//! FAT chain itself is zeroed on delete, so we assume contiguous allocation
//! (what every FAT recovery tool does) and score by whether the clusters are
//! currently free.
//!
//! exFAT: deleted entries keep their full type byte with bit 7 (in-use)
//! cleared; the NoFatChain flag tells us whether the data was contiguous.

use super::{AllocBitmap, MetadataScan, Volume};
use crate::device::BlockDevice;
use crate::model::{Extent, FoundFile, FsKind, Quality, ScanMode, ScanRequest, Source};
use crate::progress::{Phase, SharedProgress};
use crate::util::{ext_of, le_u16, le_u32, le_u64, name_looks_damaged, normalize_path, utf16le};
use crate::{Error, Result};
use std::collections::HashSet;
use std::sync::Arc;

/// Classify a generic FAT boot sector by cluster count (as the spec mandates).
pub fn classify_fat(b: &[u8]) -> FsKind {
    let bps = le_u16(b, 11) as u64;
    let spc = b[13] as u64;
    let reserved = le_u16(b, 14) as u64;
    let nfats = b[16] as u64;
    let root_entries = le_u16(b, 17) as u64;
    let total16 = le_u16(b, 19) as u64;
    let fatsz16 = le_u16(b, 22) as u64;
    let total32 = le_u32(b, 32) as u64;
    let fatsz32 = le_u32(b, 36) as u64;
    let total = if total16 != 0 { total16 } else { total32 };
    let fatsz = if fatsz16 != 0 { fatsz16 } else { fatsz32 };
    if bps == 0 || spc == 0 || fatsz == 0 || total == 0 {
        return FsKind::Unknown;
    }
    let root_sectors = (root_entries * 32).div_ceil(bps);
    let data_sectors = total.saturating_sub(reserved + nfats * fatsz + root_sectors);
    let clusters = data_sectors / spc;
    if clusters < 4085 {
        FsKind::Fat12
    } else if clusters < 65525 {
        FsKind::Fat16
    } else {
        FsKind::Fat32
    }
}

// ---------------------------------------------------------------------------
// FAT12/16/32
// ---------------------------------------------------------------------------

pub struct FatVolume {
    dev: Arc<dyn BlockDevice>,
    kind: FsKind,
    bps: u64,
    cluster_size: u64,
    fat_offset: u64,
    fat_size: u64,
    root_dir_offset: u64, // FAT12/16 fixed root
    root_dir_sectors: u64,
    root_cluster: u32, // FAT32
    data_offset: u64,
    total_clusters: u64,
    label: Option<String>,
}

#[derive(Debug)]
struct DirEntry {
    name: String,
    is_dir: bool,
    deleted: bool,
    first_cluster: u32,
    size: u64,
    mtime: Option<i64>,
    ctime: Option<i64>,
    damaged: bool,
}

impl FatVolume {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let b = dev.read_vec(0, 512)?;
        let kind = classify_fat(&b);
        if kind == FsKind::Unknown {
            return Err(Error::UnknownFilesystem);
        }
        let bps = le_u16(&b, 11) as u64;
        let spc = b[13] as u64;
        let reserved = le_u16(&b, 14) as u64;
        let nfats = b[16] as u64;
        let root_entries = le_u16(&b, 17) as u64;
        let total16 = le_u16(&b, 19) as u64;
        let fatsz16 = le_u16(&b, 22) as u64;
        let total32 = le_u32(&b, 32) as u64;
        let fatsz32 = le_u32(&b, 36) as u64;
        let total = if total16 != 0 { total16 } else { total32 };
        let fatsz = if fatsz16 != 0 { fatsz16 } else { fatsz32 };
        let root_dir_sectors = (root_entries * 32).div_ceil(bps);
        let root_dir_offset = (reserved + nfats * fatsz) * bps;
        let data_offset = root_dir_offset + root_dir_sectors * bps;
        let data_sectors = total.saturating_sub(reserved + nfats * fatsz + root_dir_sectors);
        let root_cluster = if kind == FsKind::Fat32 {
            le_u32(&b, 44)
        } else {
            0
        };
        let label_off = if kind == FsKind::Fat32 { 71 } else { 43 };
        let label = String::from_utf8_lossy(&b[label_off..label_off + 11])
            .trim()
            .to_string();
        Ok(Self {
            dev,
            kind,
            bps,
            cluster_size: bps * spc,
            fat_offset: reserved * bps,
            fat_size: fatsz * bps,
            root_dir_offset,
            root_dir_sectors,
            root_cluster,
            data_offset,
            total_clusters: data_sectors / spc,
            label: if label.is_empty() || label == "NO NAME" {
                None
            } else {
                Some(label)
            },
        })
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.data_offset + (c as u64).saturating_sub(2) * self.cluster_size
    }

    fn read_fat(&self) -> Result<Vec<u8>> {
        self.dev
            .read_vec(self.fat_offset, self.fat_size.min(64 << 20) as usize)
    }

    fn fat_entry(&self, fat: &[u8], c: u32) -> u32 {
        match self.kind {
            FsKind::Fat12 => {
                let i = (c as usize * 3) / 2;
                if i + 1 >= fat.len() {
                    return 0x0FFF;
                }
                let v = le_u16(fat, i);
                if c & 1 == 1 {
                    (v >> 4) as u32
                } else {
                    (v & 0x0FFF) as u32
                }
            }
            FsKind::Fat16 => {
                let i = c as usize * 2;
                if i + 1 >= fat.len() {
                    return 0xFFFF;
                }
                le_u16(fat, i) as u32
            }
            _ => {
                let i = c as usize * 4;
                if i + 3 >= fat.len() {
                    return 0x0FFF_FFFF;
                }
                le_u32(fat, i) & 0x0FFF_FFFF
            }
        }
    }

    fn is_eoc(&self, v: u32) -> bool {
        match self.kind {
            FsKind::Fat12 => v >= 0xFF8,
            FsKind::Fat16 => v >= 0xFFF8,
            _ => v >= 0x0FFF_FFF8,
        }
    }

    /// Follow a live chain (for directories).
    fn chain(&self, fat: &[u8], first: u32) -> Vec<u32> {
        let mut out = vec![];
        let mut c = first;
        let mut seen = HashSet::new();
        while c >= 2
            && (c as u64) < self.total_clusters + 2
            && !self.is_eoc(c)
            && seen.insert(c)
            && out.len() < 1 << 20
        {
            out.push(c);
            c = self.fat_entry(fat, c);
        }
        out
    }

    fn read_dir_bytes(&self, fat: &[u8], first_cluster: u32, is_root16: bool) -> Vec<u8> {
        if is_root16 {
            return self
                .dev
                .read_vec(
                    self.root_dir_offset,
                    (self.root_dir_sectors * self.bps) as usize,
                )
                .unwrap_or_default();
        }
        let mut buf = vec![];
        for c in self.chain(fat, first_cluster) {
            if let Ok(v) = self
                .dev
                .read_vec(self.cluster_offset(c), self.cluster_size as usize)
            {
                buf.extend(v);
            }
            if buf.len() > 64 << 20 {
                break;
            }
        }
        buf
    }

    fn parse_dir(&self, buf: &[u8]) -> Vec<DirEntry> {
        let mut out = vec![];
        let mut lfn: Vec<(u8, String)> = vec![];
        let mut lfn_deleted = false;
        for e in buf.as_chunks::<32>().0 {
            let first = e[0];
            if first == 0x00 {
                break;
            }
            let attr = e[11];
            if attr & 0x0F == 0x0F {
                // LFN piece. Deleted LFN pieces have 0xE5 but keep the rest.
                let seq = if first == 0xE5 { 0 } else { first & 0x1F };
                let mut s = String::new();
                s.push_str(&utf16le(&e[1..11]));
                s.push_str(&utf16le(&e[14..26]));
                s.push_str(&utf16le(&e[28..32]));
                let s: String = s
                    .chars()
                    .take_while(|c| *c != '\0' && *c != '\u{FFFF}')
                    .collect();
                if first == 0xE5 {
                    lfn_deleted = true;
                }
                lfn.push((seq, s));
                continue;
            }
            if attr & 0x08 != 0 {
                lfn.clear();
                continue; // volume label
            }
            let deleted = first == 0xE5;
            let short = short_name(e);
            let (name, damaged) = if !lfn.is_empty() {
                // Pieces are stored last-first; when deleted their sequence
                // numbers are gone so we rely on storage order.
                let mut pieces = lfn.clone();
                if !lfn_deleted {
                    pieces.sort_by_key(|p| p.0);
                } else {
                    pieces.reverse();
                }
                let n: String = pieces.into_iter().map(|p| p.1).collect();
                (n.clone(), name_looks_damaged(&n))
            } else {
                let dmg = deleted && short.starts_with('\u{FFFD}') || name_looks_damaged(&short);
                (short, dmg)
            };
            lfn.clear();
            lfn_deleted = false;
            let is_dir = attr & 0x10 != 0;
            if !deleted && (name == "." || name == "..") {
                continue;
            }
            let hi = if self.kind == FsKind::Fat32 {
                le_u16(e, 20) as u32
            } else {
                0
            };
            let first_cluster = (hi << 16) | le_u16(e, 26) as u32;
            out.push(DirEntry {
                name,
                is_dir,
                deleted,
                first_cluster,
                size: le_u32(e, 28) as u64,
                mtime: dos_time(le_u16(e, 24), le_u16(e, 22)),
                ctime: dos_time(le_u16(e, 16), le_u16(e, 14)),
                damaged,
            });
        }
        out
    }

    fn build_bitmap(&self, fat: &[u8]) -> AllocBitmap {
        let mut bm = AllocBitmap::new(self.cluster_size, self.total_clusters);
        for c in 0..self.total_clusters {
            let v = self.fat_entry(fat, c as u32 + 2);
            bm.set(c, v != 0);
        }
        bm
    }
}

fn short_name(e: &[u8]) -> String {
    // 8.3 names are OEM code page bytes; map non-ASCII to a placeholder.
    let dec = |b: &[u8]| -> String {
        b.iter()
            .map(|c| if c.is_ascii() { *c as char } else { '_' })
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    let base = dec(&e[0..8]);
    let ext = dec(&e[8..11]);
    let mut base = base;
    if e[0] == 0xE5 || e[0] == 0x05 {
        // First char lost on delete; show placeholder.
        base = format!("_{}", base.chars().skip(1).collect::<String>());
    }
    let lower = e[12] & 0x08 != 0;
    let lower_ext = e[12] & 0x10 != 0;
    let mut n = if lower { base.to_lowercase() } else { base };
    if !ext.is_empty() {
        n.push('.');
        n.push_str(&if lower_ext { ext.to_lowercase() } else { ext });
    }
    n
}

fn dos_time(date: u16, time: u16) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let y = 1980 + (date >> 9) as i64;
    let m = ((date >> 5) & 0xF) as i64;
    let d = (date & 0x1F) as i64;
    let hh = (time >> 11) as i64;
    let mm = ((time >> 5) & 0x3F) as i64;
    let ss = ((time & 0x1F) * 2) as i64;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86400 + hh * 3600 + mm * 60 + ss)
}

/// Howard Hinnant's days_from_civil.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

impl Volume for FatVolume {
    fn kind(&self) -> FsKind {
        self.kind
    }
    fn label(&self) -> Option<String> {
        self.label.clone()
    }
    fn cluster_size(&self) -> u64 {
        self.cluster_size
    }
    fn total_clusters(&self) -> u64 {
        self.total_clusters
    }

    fn scan_metadata(&self, req: &ScanRequest, progress: &SharedProgress) -> Result<MetadataScan> {
        progress.set_phase(Phase::Metadata, 0);
        progress.set_message("读取 FAT");
        let fat = self.read_fat()?;
        let bitmap = self.build_bitmap(&fat);
        let target = req.directory.as_deref().map(normalize_path);

        // BFS over live + deleted directories. Deleted directories keep their
        // first cluster; we read them as one cluster (chain is gone).
        let mut queue: Vec<(String, u32, bool, bool)> = vec![(
            String::new(),
            self.root_cluster,
            self.kind != FsKind::Fat32,
            false,
        )];
        let mut visited = HashSet::new();
        let mut files = vec![];
        let mut id = 1u64;
        let mut dirs_scanned = 0u64;
        while let Some((path, cluster, is_root16, dir_deleted)) = queue.pop() {
            progress.checkpoint()?;
            if !is_root16 && !visited.insert(cluster) {
                continue;
            }
            dirs_scanned += 1;
            progress.set_done(dirs_scanned);
            let buf = if dir_deleted {
                self.dev
                    .read_vec(self.cluster_offset(cluster), self.cluster_size as usize)
                    .unwrap_or_default()
            } else {
                self.read_dir_bytes(&fat, cluster, is_root16)
            };
            for e in self.parse_dir(&buf) {
                if e.is_dir {
                    if e.first_cluster >= 2 && (e.first_cluster as u64) < self.total_clusters + 2 {
                        let p = if path.is_empty() {
                            e.name.clone()
                        } else {
                            format!("{}/{}", path, e.name)
                        };
                        queue.push((p, e.first_cluster, false, dir_deleted || e.deleted));
                    }
                    continue;
                }
                if !(e.deleted || dir_deleted) {
                    continue;
                }
                if e.damaged && !req.include_damaged_names {
                    continue;
                }
                if req.mode == ScanMode::Directory {
                    let Some(t) = &target else { continue };
                    if !crate::model::path_has_prefix(&path, t) {
                        continue;
                    }
                }
                let ext = ext_of(&e.name);
                if !req.extensions.is_empty()
                    && !req.extensions.iter().any(|x| x.eq_ignore_ascii_case(&ext))
                {
                    continue;
                }
                let extents = if e.size > 0 && e.first_cluster >= 2 {
                    vec![Extent {
                        offset: self.cluster_offset(e.first_cluster),
                        len: e.size,
                    }]
                } else {
                    vec![]
                };
                let in_range = extents.iter().all(|x| x.offset + x.len <= self.dev.size());
                let ov = bitmap.overwritten_pct(&extents);
                let (quality, note) = if !in_range {
                    (Quality::Low, "起始簇超出卷范围".into())
                } else if e.size == 0 {
                    (Quality::Low, "文件大小为 0".into())
                } else if ov == 0 {
                    (Quality::Medium, "假定连续存储；数据簇当前空闲".into())
                } else {
                    (Quality::Low, format!("{}% 数据簇已被重新分配", ov))
                };
                let quality = if quality == Quality::Medium
                    && extents
                        .first()
                        .map(|x| x.len <= self.cluster_size)
                        .unwrap_or(false)
                {
                    Quality::High // single cluster cannot be fragmented
                } else {
                    quality
                };
                files.push(FoundFile {
                    id,
                    name: e.name,
                    dir: Some(path.clone()),
                    size: e.size,
                    source: Source::Metadata,
                    quality,
                    overwritten_pct: ov,
                    deleted_at: None,
                    modified_at: e.mtime,
                    created_at: e.ctime,
                    ext,
                    extents,
                    resident: None,
                    name_damaged: e.damaged,
                    note,
                });
                id += 1;
                progress.add_found(1);
            }
        }
        Ok(MetadataScan {
            files,
            bitmap,
            volume_label: self.label.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// exFAT
// ---------------------------------------------------------------------------

pub struct ExFatVolume {
    dev: Arc<dyn BlockDevice>,
    cluster_size: u64,
    fat_offset: u64,
    fat_len: u64,
    heap_offset: u64,
    total_clusters: u64,
    root_cluster: u32,
    label: Option<String>,
}

impl ExFatVolume {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let b = dev.read_vec(0, 512)?;
        if &b[3..11] != b"EXFAT   " {
            return Err(Error::UnknownFilesystem);
        }
        let fat_offset_sec = le_u32(&b, 80) as u64;
        let fat_len_sec = le_u32(&b, 84) as u64;
        let heap_sec = le_u32(&b, 88) as u64;
        let cluster_count = le_u32(&b, 92) as u64;
        let root_cluster = le_u32(&b, 96);
        let bps = 1u64 << b[108];
        let spc = 1u64 << b[109];
        if b[108] < 9 || b[108] > 12 || b[109] > 25 {
            return Err(Error::Corrupt("exFAT 引导扇区参数无效".into()));
        }
        Ok(Self {
            dev,
            cluster_size: bps * spc,
            fat_offset: fat_offset_sec * bps,
            fat_len: fat_len_sec * bps,
            heap_offset: heap_sec * bps,
            total_clusters: cluster_count,
            root_cluster,
            label: None,
        })
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.heap_offset + (c as u64).saturating_sub(2) * self.cluster_size
    }

    fn fat_next(&self, fat: &[u8], c: u32) -> u32 {
        let i = c as usize * 4;
        if i + 3 >= fat.len() {
            return 0xFFFF_FFFF;
        }
        le_u32(fat, i)
    }

    fn chain(&self, fat: &[u8], first: u32, no_fat_chain: bool, size: u64) -> Vec<u32> {
        if no_fat_chain {
            let n = size.div_ceil(self.cluster_size);
            return (0..n as u32).map(|i| first + i).collect();
        }
        let mut out = vec![];
        let mut c = first;
        let mut seen = HashSet::new();
        while c >= 2
            && (c as u64) < self.total_clusters + 2
            && c < 0xFFFF_FFF7
            && seen.insert(c)
            && out.len() < 1 << 20
        {
            out.push(c);
            c = self.fat_next(fat, c);
        }
        out
    }
}

struct ExEntry {
    name: String,
    is_dir: bool,
    deleted: bool,
    first_cluster: u32,
    size: u64,
    no_fat_chain: bool,
    mtime: Option<i64>,
    ctime: Option<i64>,
    damaged: bool,
}

fn exfat_time(ts: u32) -> Option<i64> {
    if ts == 0 {
        return None;
    }
    dos_time((ts >> 16) as u16, (ts & 0xFFFF) as u16)
}

impl ExFatVolume {
    fn parse_dir(&self, buf: &[u8]) -> (Vec<ExEntry>, Option<String>, Option<(u32, u64)>) {
        let mut out = vec![];
        let mut label = None;
        let mut bitmap_loc = None;
        let mut i = 0;
        while i + 32 <= buf.len() {
            let e = &buf[i..i + 32];
            let t = e[0];
            if t == 0 {
                break;
            }
            let in_use = t & 0x80 != 0;
            let base = t & 0x7F;
            match base {
                0x03 if in_use => {
                    let n = e[1] as usize;
                    label = Some(utf16le(&e[2..2 + (n * 2).min(22)]));
                }
                0x01 if in_use => {
                    bitmap_loc = Some((le_u32(e, 20), le_u64(e, 24)));
                }
                0x05 => {
                    // File directory entry followed by stream ext + name exts.
                    let secondary = e[1] as usize;
                    let attr = le_u16(e, 4);
                    let is_dir = attr & 0x10 != 0;
                    let ctime = exfat_time(le_u32(e, 8));
                    let mtime = exfat_time(le_u32(e, 12));
                    let mut name = String::new();
                    let mut first_cluster = 0;
                    let mut size = 0;
                    let mut no_fat_chain = false;
                    let mut name_len = 0usize;
                    let mut j = i + 32;
                    for _ in 0..secondary {
                        if j + 32 > buf.len() {
                            break;
                        }
                        let s = &buf[j..j + 32];
                        match s[0] & 0x7F {
                            0x40 => {
                                no_fat_chain = s[1] & 0x02 != 0;
                                name_len = s[3] as usize;
                                first_cluster = le_u32(s, 20);
                                size = le_u64(s, 24);
                            }
                            0x41 => name.push_str(&utf16le(&s[2..32])),
                            _ => {}
                        }
                        j += 32;
                    }
                    let name: String = name.chars().take(name_len).collect();
                    let damaged = name_looks_damaged(&name);
                    out.push(ExEntry {
                        name,
                        is_dir,
                        deleted: !in_use,
                        first_cluster,
                        size,
                        no_fat_chain,
                        mtime,
                        ctime,
                        damaged,
                    });
                    i = j;
                    continue;
                }
                _ => {}
            }
            i += 32;
        }
        (out, label, bitmap_loc)
    }
}

impl Volume for ExFatVolume {
    fn kind(&self) -> FsKind {
        FsKind::ExFat
    }
    fn label(&self) -> Option<String> {
        self.label.clone()
    }
    fn cluster_size(&self) -> u64 {
        self.cluster_size
    }
    fn total_clusters(&self) -> u64 {
        self.total_clusters
    }

    fn scan_metadata(&self, req: &ScanRequest, progress: &SharedProgress) -> Result<MetadataScan> {
        progress.set_phase(Phase::Metadata, 0);
        progress.set_message("读取 FAT 与分配位图");
        let fat = self
            .dev
            .read_vec(self.fat_offset, self.fat_len.min(64 << 20) as usize)?;
        let target = req.directory.as_deref().map(normalize_path);

        // Root dir first to find the allocation bitmap.
        let root_buf = {
            let mut b = vec![];
            for c in self.chain(&fat, self.root_cluster, false, 0) {
                if let Ok(v) = self
                    .dev
                    .read_vec(self.cluster_offset(c), self.cluster_size as usize)
                {
                    b.extend(v);
                }
            }
            b
        };
        let (_, label, bitmap_loc) = self.parse_dir(&root_buf);
        let bitmap = match bitmap_loc {
            Some((c, len)) => {
                let raw = self
                    .dev
                    .read_vec(self.cluster_offset(c), len.min(64 << 20) as usize)
                    .unwrap_or_default();
                AllocBitmap::from_raw(self.cluster_size, self.total_clusters, raw)
            }
            None => AllocBitmap::all_allocated(self.cluster_size, self.total_clusters),
        };

        let mut queue: Vec<(String, u32, bool, u64, bool)> =
            vec![(String::new(), self.root_cluster, false, 0, false)];
        let mut visited = HashSet::new();
        let mut files = vec![];
        let mut id = 1u64;
        let mut n = 0u64;
        while let Some((path, cluster, nfc, size, dir_deleted)) = queue.pop() {
            progress.checkpoint()?;
            if !visited.insert(cluster) {
                continue;
            }
            n += 1;
            progress.set_done(n);
            let buf = {
                let mut b = vec![];
                let chain = if dir_deleted {
                    vec![cluster]
                } else {
                    self.chain(&fat, cluster, nfc, size)
                };
                for c in chain {
                    if let Ok(v) = self
                        .dev
                        .read_vec(self.cluster_offset(c), self.cluster_size as usize)
                    {
                        b.extend(v);
                    }
                    if b.len() > 64 << 20 {
                        break;
                    }
                }
                b
            };
            let (entries, _, _) = self.parse_dir(&buf);
            for e in entries {
                if e.is_dir {
                    if e.first_cluster >= 2 {
                        let p = if path.is_empty() {
                            e.name.clone()
                        } else {
                            format!("{}/{}", path, e.name)
                        };
                        queue.push((
                            p,
                            e.first_cluster,
                            e.no_fat_chain,
                            e.size,
                            dir_deleted || e.deleted,
                        ));
                    }
                    continue;
                }
                if !(e.deleted || dir_deleted) {
                    continue;
                }
                if e.damaged && !req.include_damaged_names {
                    continue;
                }
                if req.mode == ScanMode::Directory {
                    let Some(t) = &target else { continue };
                    if !crate::model::path_has_prefix(&path, t) {
                        continue;
                    }
                }
                let ext = ext_of(&e.name);
                if !req.extensions.is_empty()
                    && !req.extensions.iter().any(|x| x.eq_ignore_ascii_case(&ext))
                {
                    continue;
                }
                let extents = if e.size > 0 && e.first_cluster >= 2 {
                    // Contiguous when NoFatChain; otherwise follow FAT if still intact.
                    let chain = self.chain(&fat, e.first_cluster, e.no_fat_chain, e.size);
                    let mut ex: Vec<Extent> = vec![];
                    let mut remaining = e.size;
                    for c in chain {
                        if remaining == 0 {
                            break;
                        }
                        let l = remaining.min(self.cluster_size);
                        let off = self.cluster_offset(c);
                        if let Some(last) = ex.last_mut() {
                            if last.offset + last.len == off {
                                last.len += l;
                                remaining -= l;
                                continue;
                            }
                        }
                        ex.push(Extent {
                            offset: off,
                            len: l,
                        });
                        remaining -= l;
                    }
                    if ex.is_empty() {
                        vec![Extent {
                            offset: self.cluster_offset(e.first_cluster),
                            len: e.size,
                        }]
                    } else {
                        ex
                    }
                } else {
                    vec![]
                };
                let in_range = extents.iter().all(|x| x.offset + x.len <= self.dev.size());
                let ov = bitmap.overwritten_pct(&extents);
                let (quality, note) = if !in_range {
                    (Quality::Low, "起始簇超出卷范围".into())
                } else if e.size == 0 {
                    (Quality::Low, "文件大小为 0".into())
                } else if ov == 0 {
                    (
                        if e.no_fat_chain {
                            Quality::High
                        } else {
                            Quality::Medium
                        },
                        if e.no_fat_chain {
                            "连续存储，数据簇空闲".into()
                        } else {
                            "FAT 链可能已清除，假定连续".into()
                        },
                    )
                } else {
                    (Quality::Low, format!("{}% 数据簇已被重新分配", ov))
                };
                files.push(FoundFile {
                    id,
                    name: e.name,
                    dir: Some(path.clone()),
                    size: e.size,
                    source: Source::Metadata,
                    quality,
                    overwritten_pct: ov,
                    deleted_at: None,
                    modified_at: e.mtime,
                    created_at: e.ctime,
                    ext,
                    extents,
                    resident: None,
                    name_damaged: e.damaged,
                    note,
                });
                id += 1;
                progress.add_found(1);
            }
        }
        Ok(MetadataScan {
            files,
            bitmap,
            volume_label: label.or_else(|| self.label.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
    }

    #[test]
    fn dos_time_decode() {
        // 2025-05-02 14:21:00  date = (45<<9)|(5<<5)|2, time = (14<<11)|(21<<5)|0
        let d = (45u16 << 9) | (5 << 5) | 2;
        let t = (14u16 << 11) | (21 << 5);
        assert_eq!(dos_time(d, t), Some(1_746_195_660));
    }
}
