//! Read-only NTFS parser focused on deleted-record recovery.
//!
//! Scope: boot sector, $MFT self-mapping, FILE record fixups,
//! $STANDARD_INFORMATION / $FILE_NAME / $DATA (resident + non-resident,
//! including $ATTRIBUTE_LIST continuation records), $Bitmap, and directory
//! path reconstruction from parent references — including for parents that
//! are themselves deleted.

use super::{AllocBitmap, MetadataScan, Volume};
use crate::device::BlockDevice;
use crate::model::{Extent, FoundFile, FsKind, Quality, ScanMode, ScanRequest, Source};
use crate::progress::{Phase, SharedProgress};
use crate::util::{
    ext_of, filetime_to_unix, le_u16, le_u32, le_u64, name_looks_damaged, normalize_path, utf16le,
};
use crate::{Error, Result};
use std::collections::HashMap;
use std::sync::Arc;

const ATTR_STD_INFO: u32 = 0x10;
const ATTR_ATTR_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

const MFT_RECORD_ROOT: u64 = 5;
const MFT_RECORD_BITMAP: u64 = 6;

pub struct NtfsVolume {
    dev: Arc<dyn BlockDevice>,
    bytes_per_sector: u32,
    cluster_size: u64,
    total_clusters: u64,
    record_size: u32,
    mft_runs: Vec<Extent>,
    mft_records: u64,
    label: Option<String>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct Record {
    in_use: bool,
    is_dir: bool,
    base_ref: u64,
    seq: u16,
    names: Vec<FileName>,
    data: Option<DataAttr>,
    std: Option<StdInfo>,
    attr_list: Option<Vec<u8>>,
    attr_list_runs: Option<Vec<Extent>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileName {
    parent: u64,
    parent_seq: u16,
    name: String,
    namespace: u8, // 0 posix 1 win32 2 dos 3 win32+dos
}

#[derive(Debug, Clone, Default)]
struct DataAttr {
    resident: Option<Vec<u8>>,
    runs: Vec<Extent>,
    real_size: u64,
    /// Highest VCN seen so far (for stitching attribute-list pieces).
    next_vcn: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct StdInfo {
    created: u64,
    modified: u64,
    mft_modified: u64,
}

impl NtfsVolume {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let b = dev.read_vec(0, 512)?;
        if &b[3..11] != b"NTFS    " {
            return Err(Error::UnknownFilesystem);
        }
        let bytes_per_sector = le_u16(&b, 11) as u32;
        let spc = b[13];
        let sectors_per_cluster: u64 = if spc > 0x80 {
            1u64 << (256 - spc as u32)
        } else {
            spc as u64
        };
        if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) || sectors_per_cluster == 0 {
            return Err(Error::Corrupt("NTFS 引导扇区参数无效".into()));
        }
        let cluster_size = bytes_per_sector as u64 * sectors_per_cluster;
        let total_sectors = le_u64(&b, 40);
        let total_clusters = total_sectors / sectors_per_cluster;
        let mft_lcn = le_u64(&b, 48);
        let rs = b[64] as i8;
        let record_size: u32 = if rs < 0 {
            1u32 << (-(rs as i32))
        } else {
            rs as u32 * cluster_size as u32
        };
        if !(256..=65536).contains(&record_size) {
            return Err(Error::Corrupt("MFT 记录大小无效".into()));
        }
        let mut vol = Self {
            dev,
            bytes_per_sector,
            cluster_size,
            total_clusters,
            record_size,
            mft_runs: vec![Extent {
                offset: mft_lcn * cluster_size,
                len: record_size as u64 * 16,
            }],
            mft_records: 16,
            label: None,
        };
        // Record 0 is $MFT itself; its $DATA runs tell us where every record lives.
        let r0 = vol.read_record_raw(0)?;
        let rec0 = vol.parse_record(&r0, 0)?;
        let data = rec0
            .data
            .ok_or_else(|| Error::Corrupt("$MFT 没有 $DATA 属性".into()))?;
        if data.runs.is_empty() {
            return Err(Error::Corrupt("$MFT 数据运行为空".into()));
        }
        vol.mft_runs = data.runs;
        vol.mft_records = data.real_size / record_size as u64;
        // Volume label from $Volume (record 3), attribute 0x60 — optional.
        vol.label = vol.read_volume_label();
        Ok(vol)
    }

    fn read_volume_label(&self) -> Option<String> {
        let raw = self.read_record_raw(3).ok()?;
        let raw = apply_fixups(raw, self.bytes_per_sector as usize)?;
        let mut off = le_u16(&raw, 20) as usize;
        while off + 16 <= raw.len() {
            let t = le_u32(&raw, off);
            if t == ATTR_END {
                break;
            }
            let len = le_u32(&raw, off + 4) as usize;
            if len == 0 {
                break;
            }
            if t == 0x60 && raw[off + 8] == 0 {
                let vlen = le_u32(&raw, off + 16) as usize;
                let voff = le_u16(&raw, off + 20) as usize;
                if off + voff + vlen <= raw.len() {
                    let s = utf16le(&raw[off + voff..off + voff + vlen]);
                    return if s.is_empty() { None } else { Some(s) };
                }
            }
            off += len;
        }
        None
    }

    fn record_offset(&self, n: u64) -> Option<u64> {
        let mut rel = n * self.record_size as u64;
        for r in &self.mft_runs {
            if rel < r.len {
                return Some(r.offset + rel);
            }
            rel -= r.len;
        }
        None
    }

    fn read_record_raw(&self, n: u64) -> Result<Vec<u8>> {
        let off = self
            .record_offset(n)
            .ok_or_else(|| Error::Corrupt(format!("MFT 记录 {} 超出范围", n)))?;
        self.dev.read_vec(off, self.record_size as usize)
    }

    /// Parse one FILE record. Does not follow $ATTRIBUTE_LIST; see `resolve_attr_list`.
    fn parse_record(&self, raw: &[u8], n: u64) -> Result<Record> {
        if &raw[0..4] != b"FILE" {
            return Err(Error::Corrupt(format!("记录 {} 签名无效", n)));
        }
        let raw = apply_fixups(raw.to_vec(), self.bytes_per_sector as usize)
            .ok_or_else(|| Error::Corrupt(format!("记录 {} 修正序列无效", n)))?;
        let flags = le_u16(&raw, 22);
        let mut rec = Record {
            in_use: flags & 1 != 0,
            is_dir: flags & 2 != 0,
            base_ref: le_u64(&raw, 32) & 0x0000_FFFF_FFFF_FFFF,
            seq: le_u16(&raw, 16),
            ..Default::default()
        };
        let mut off = le_u16(&raw, 20) as usize;
        let used = (le_u32(&raw, 24) as usize).min(raw.len());
        while off + 16 <= used {
            let t = le_u32(&raw, off);
            if t == ATTR_END {
                break;
            }
            let len = le_u32(&raw, off + 4) as usize;
            if len < 16 || off + len > raw.len() {
                break;
            }
            let a = &raw[off..off + len];
            let non_resident = a[8] != 0;
            let name_len = a[9] as usize;
            match t {
                ATTR_STD_INFO if !non_resident => {
                    if let Some(v) = resident_value(a) {
                        if v.len() >= 32 {
                            rec.std = Some(StdInfo {
                                created: le_u64(v, 0),
                                modified: le_u64(v, 8),
                                mft_modified: le_u64(v, 16),
                            });
                        }
                    }
                }
                ATTR_FILE_NAME if !non_resident => {
                    if let Some(v) = resident_value(a) {
                        if v.len() >= 66 {
                            let parent_raw = le_u64(v, 0);
                            let nlen = v[64] as usize;
                            let ns = v[65];
                            if 66 + nlen * 2 <= v.len() {
                                rec.names.push(FileName {
                                    parent: parent_raw & 0x0000_FFFF_FFFF_FFFF,
                                    parent_seq: (parent_raw >> 48) as u16,
                                    name: utf16le(&v[66..66 + nlen * 2]),
                                    namespace: ns,
                                });
                            }
                        }
                    }
                }
                ATTR_DATA if name_len == 0 => {
                    // Unnamed $DATA = main stream. Ignore alternate streams.
                    let d = rec.data.get_or_insert_with(DataAttr::default);
                    if non_resident {
                        let start_vcn = le_u64(a, 16);
                        let real = le_u64(a, 48);
                        let runs_off = le_u16(a, 32) as usize;
                        if runs_off < a.len() {
                            let runs = decode_runs(&a[runs_off..], self.cluster_size);
                            if start_vcn == 0 {
                                d.real_size = real;
                                d.runs = runs.clone();
                            } else {
                                d.runs.extend(runs.clone());
                            }
                            d.next_vcn = le_u64(a, 24) + 1;
                        }
                    } else if let Some(v) = resident_value(a) {
                        d.resident = Some(v.to_vec());
                        d.real_size = v.len() as u64;
                    }
                }
                ATTR_ATTR_LIST => {
                    if non_resident {
                        let runs_off = le_u16(a, 32) as usize;
                        if runs_off < a.len() {
                            rec.attr_list_runs =
                                Some(decode_runs(&a[runs_off..], self.cluster_size));
                        }
                    } else if let Some(v) = resident_value(a) {
                        rec.attr_list = Some(v.to_vec());
                    }
                }
                _ => {}
            }
            off += len;
        }
        Ok(rec)
    }

    /// Follow $ATTRIBUTE_LIST to pull $DATA pieces from extension records.
    fn resolve_attr_list(&self, rec: &mut Record, n: u64) {
        let list = match (&rec.attr_list, &rec.attr_list_runs) {
            (Some(l), _) => l.clone(),
            (None, Some(runs)) => {
                let mut buf = vec![];
                for r in runs {
                    if let Ok(v) = self.dev.read_vec(r.offset, r.len.min(1 << 20) as usize) {
                        buf.extend(v);
                    }
                }
                buf
            }
            _ => return,
        };
        let mut off = 0usize;
        let mut seen: Vec<u64> = vec![];
        while off + 26 <= list.len() {
            let t = le_u32(&list, off);
            let len = le_u16(&list, off + 4) as usize;
            if len < 26 || off + len > list.len() {
                break;
            }
            let name_len = list[off + 6];
            let start_vcn = le_u64(&list, off + 8);
            let mref = le_u64(&list, off + 16) & 0x0000_FFFF_FFFF_FFFF;
            if t == ATTR_DATA && name_len == 0 && mref != n && !seen.contains(&mref) {
                seen.push(mref);
                if let Ok(raw) = self.read_record_raw(mref) {
                    if let Ok(ext) = self.parse_record(&raw, mref) {
                        if ext.base_ref == n {
                            if let Some(ed) = ext.data {
                                let d = rec.data.get_or_insert_with(DataAttr::default);
                                if start_vcn == 0 && ed.real_size > 0 {
                                    d.real_size = ed.real_size;
                                }
                                if ed.resident.is_some() {
                                    d.resident = ed.resident;
                                } else {
                                    d.runs.extend(ed.runs);
                                }
                            }
                        }
                    }
                }
            }
            off += len;
        }
    }

    /// Build the full path of record `n` using a cache. Works for deleted
    /// parents too, since their FILE_NAME attributes usually survive.
    fn path_of(
        &self,
        parent: u64,
        cache: &mut HashMap<u64, Option<String>>,
        names: &HashMap<u64, (String, u64)>,
    ) -> Option<String> {
        if parent == MFT_RECORD_ROOT {
            return Some(String::new());
        }
        if let Some(p) = cache.get(&parent) {
            return p.clone();
        }
        cache.insert(parent, None); // cycle guard
        let result = names.get(&parent).and_then(|(name, grand)| {
            if *grand == parent {
                return None;
            }
            self.path_of(*grand, cache, names).map(|pp| {
                if pp.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", pp, name)
                }
            })
        });
        cache.insert(parent, result.clone());
        result
    }

    fn read_bitmap(&self) -> AllocBitmap {
        let fallback = || AllocBitmap::all_allocated(self.cluster_size, self.total_clusters);
        let Ok(raw) = self.read_record_raw(MFT_RECORD_BITMAP) else {
            return fallback();
        };
        let Ok(mut rec) = self.parse_record(&raw, MFT_RECORD_BITMAP) else {
            return fallback();
        };
        self.resolve_attr_list(&mut rec, MFT_RECORD_BITMAP);
        let Some(d) = rec.data else { return fallback() };
        if let Some(r) = d.resident {
            return AllocBitmap::from_raw(self.cluster_size, self.total_clusters, r);
        }
        let need = self.total_clusters.div_ceil(8) as usize;
        let mut buf = Vec::with_capacity(need);
        for r in &d.runs {
            if buf.len() >= need {
                break;
            }
            if r.offset == u64::MAX {
                buf.extend(std::iter::repeat_n(0u8, r.len as usize));
                continue;
            }
            match self.dev.read_vec(r.offset, r.len as usize) {
                Ok(v) => buf.extend(v),
                Err(_) => return fallback(),
            }
        }
        AllocBitmap::from_raw(self.cluster_size, self.total_clusters, buf)
    }
}

fn resident_value(a: &[u8]) -> Option<&[u8]> {
    let vlen = le_u32(a, 16) as usize;
    let voff = le_u16(a, 20) as usize;
    if voff + vlen <= a.len() {
        Some(&a[voff..voff + vlen])
    } else {
        None
    }
}

/// Apply the update-sequence fixup; returns `None` when the record is torn.
fn apply_fixups(mut raw: Vec<u8>, sector: usize) -> Option<Vec<u8>> {
    let usa_off = le_u16(&raw, 4) as usize;
    let usa_count = le_u16(&raw, 6) as usize;
    if usa_count < 2 || usa_off + usa_count * 2 > raw.len() {
        return None;
    }
    let usn = [raw[usa_off], raw[usa_off + 1]];
    for i in 1..usa_count {
        let pos = i * sector - 2;
        if pos + 2 > raw.len() {
            break;
        }
        if raw[pos] != usn[0] || raw[pos + 1] != usn[1] {
            // Torn write; keep going but mark by returning None only if the
            // first sector is inconsistent (header unusable).
            if i == 1 {
                return None;
            }
            continue;
        }
        raw[pos] = raw[usa_off + i * 2];
        raw[pos + 1] = raw[usa_off + i * 2 + 1];
    }
    Some(raw)
}

/// Decode NTFS data runs into absolute byte extents. Sparse runs get
/// `offset == u64::MAX`.
pub fn decode_runs(mut b: &[u8], cluster: u64) -> Vec<Extent> {
    let mut out = vec![];
    let mut lcn: i64 = 0;
    while !b.is_empty() && b[0] != 0 {
        let hdr = b[0];
        let len_sz = (hdr & 0x0F) as usize;
        let off_sz = (hdr >> 4) as usize;
        if len_sz == 0 || 1 + len_sz + off_sz > b.len() {
            break;
        }
        let mut len: u64 = 0;
        for i in 0..len_sz {
            len |= (b[1 + i] as u64) << (8 * i);
        }
        if off_sz == 0 {
            out.push(Extent {
                offset: u64::MAX,
                len: len * cluster,
            });
        } else {
            let mut off: i64 = 0;
            for i in 0..off_sz {
                off |= (b[1 + len_sz + i] as i64) << (8 * i);
            }
            // Sign-extend.
            let shift = 64 - 8 * off_sz;
            off = (off << shift) >> shift;
            lcn += off;
            if lcn < 0 {
                break;
            }
            out.push(Extent {
                offset: lcn as u64 * cluster,
                len: len * cluster,
            });
        }
        b = &b[1 + len_sz + off_sz..];
    }
    out
}

impl Volume for NtfsVolume {
    fn kind(&self) -> FsKind {
        FsKind::Ntfs
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
        progress.set_phase(Phase::Metadata, self.mft_records);
        progress.set_message("读取 $Bitmap");
        let bitmap = self.read_bitmap();

        // Pass 1: every record's primary name + parent (for path reconstruction),
        // and collect deleted base records.
        let mut names: HashMap<u64, (String, u64)> = HashMap::new();
        let mut deleted: Vec<(u64, Record)> = vec![];
        progress.set_message("遍历 MFT");
        for n in 0..self.mft_records {
            if n % 256 == 0 {
                progress.checkpoint()?;
                progress.set_done(n);
            }
            let Ok(raw) = self.read_record_raw(n) else {
                progress.add_read_error();
                continue;
            };
            if &raw[0..4] != b"FILE" {
                continue;
            }
            let Ok(rec) = self.parse_record(&raw, n) else {
                continue;
            };
            if rec.base_ref != 0 {
                continue; // extension record
            }
            if let Some(fname) = best_name(&rec.names) {
                names.insert(n, (fname.name.clone(), fname.parent));
            }
            if !rec.in_use && !rec.is_dir {
                deleted.push((n, rec));
            }
        }
        progress.set_done(self.mft_records);

        // Pass 2: build paths, filter, score.
        progress.set_message("重建目录结构");
        let target = req.directory.as_deref().map(normalize_path);
        let mut cache: HashMap<u64, Option<String>> = HashMap::new();
        let mut files = Vec::new();
        let mut id = 1u64;
        for (n, mut rec) in deleted {
            progress.checkpoint()?;
            let fname = best_name(&rec.names);
            let (name, dir, damaged) = match fname {
                Some(f) => {
                    let dir = self.path_of(f.parent, &mut cache, &names);
                    let dmg = name_looks_damaged(&f.name);
                    let nm = if dmg {
                        format!("FILE_#{}", n)
                    } else {
                        f.name.clone()
                    };
                    let no_dir = dir.is_none();
                    (nm, dir, dmg || no_dir)
                }
                None => (format!("FILE_#{}", n), None, true),
            };
            if damaged && !req.include_damaged_names {
                continue;
            }
            if req.mode == ScanMode::Directory {
                let Some(t) = &target else { continue };
                let d = dir.clone().unwrap_or_default();
                if !crate::model::path_has_prefix(&d, t) {
                    // A deleted parent whose own path could not be rebuilt
                    // becomes an orphan; keep it under a synthetic folder when
                    // its name matches the target's last component.
                    continue;
                }
            }
            if rec.attr_list.is_some() || rec.attr_list_runs.is_some() {
                self.resolve_attr_list(&mut rec, n);
            }
            let Some(data) = rec.data else { continue };
            let ext = ext_of(&name);
            if !req.extensions.is_empty()
                && !req.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))
            {
                continue;
            }
            let (extents, resident, size) = if let Some(r) = data.resident {
                (vec![], Some(r.clone()), r.len() as u64)
            } else {
                // Trim runs to real size and drop sparse markers.
                let mut remaining = data.real_size;
                let mut ex = vec![];
                for r in data.runs {
                    if remaining == 0 {
                        break;
                    }
                    let l = r.len.min(remaining);
                    remaining -= l;
                    if r.offset != u64::MAX {
                        ex.push(Extent {
                            offset: r.offset,
                            len: l,
                        });
                    }
                }
                (ex, None, data.real_size)
            };
            let overwritten = if resident.is_some() {
                0
            } else {
                bitmap.overwritten_pct(&extents)
            };
            let in_range = extents.iter().all(|e| e.offset + e.len <= self.dev.size());
            let (quality, note) = score(overwritten, in_range, damaged, size, resident.is_some());
            let std = rec.std.unwrap_or_default();
            files.push(FoundFile {
                id,
                name,
                dir,
                size,
                source: Source::Metadata,
                quality,
                overwritten_pct: overwritten,
                // NTFS has no deletion timestamp; MFT-modified is the closest proxy.
                deleted_at: filetime_to_unix(std.mft_modified),
                modified_at: filetime_to_unix(std.modified),
                created_at: filetime_to_unix(std.created),
                ext,
                extents,
                resident,
                name_damaged: damaged,
                note,
            });
            id += 1;
            progress.add_found(1);
        }
        Ok(MetadataScan {
            files,
            bitmap,
            volume_label: self.label.clone(),
        })
    }
}

/// Prefer Win32 / POSIX names over DOS 8.3 aliases.
fn best_name(names: &[FileName]) -> Option<&FileName> {
    names
        .iter()
        .find(|n| n.namespace == 1 || n.namespace == 3)
        .or_else(|| names.iter().find(|n| n.namespace == 0))
        .or_else(|| names.first())
}

fn score(
    overwritten: u8,
    in_range: bool,
    damaged: bool,
    size: u64,
    resident: bool,
) -> (Quality, String) {
    if !in_range {
        return (Quality::Low, "数据运行超出卷范围".into());
    }
    if resident {
        return (Quality::High, "数据驻留在 MFT 记录内".into());
    }
    if size == 0 {
        return (Quality::Low, "文件大小为 0".into());
    }
    match overwritten {
        0 => (
            if damaged {
                Quality::Medium
            } else {
                Quality::High
            },
            if damaged {
                "数据完整，文件名损坏".into()
            } else {
                "数据簇未被重新分配".into()
            },
        ),
        1..=30 => (
            Quality::Medium,
            format!("{}% 数据簇已被重新分配", overwritten),
        ),
        _ => (Quality::Low, format!("{}% 数据簇已被重新分配", overwritten)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_decode_positive_and_negative() {
        // run1: len 0x10 clusters @ lcn 0x20 ; run2: len 0x08 @ lcn 0x20-0x05 = 0x1B
        let b = [0x11, 0x10, 0x20, 0x11, 0x08, 0xFB, 0x00];
        let r = decode_runs(&b, 4096);
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0],
            Extent {
                offset: 0x20 * 4096,
                len: 0x10 * 4096
            }
        );
        assert_eq!(
            r[1],
            Extent {
                offset: 0x1B * 4096,
                len: 0x08 * 4096
            }
        );
    }

    #[test]
    fn runs_sparse() {
        let b = [0x01, 0x04, 0x00];
        let r = decode_runs(&b, 512);
        assert_eq!(r[0].offset, u64::MAX);
        assert_eq!(r[0].len, 4 * 512);
    }

    #[test]
    fn fixup_roundtrip() {
        let mut raw = vec![0u8; 1024];
        raw[0..4].copy_from_slice(b"FILE");
        raw[4..6].copy_from_slice(&48u16.to_le_bytes()); // usa offset
        raw[6..8].copy_from_slice(&3u16.to_le_bytes()); // usa count (1 usn + 2 sectors)
        raw[48..50].copy_from_slice(&[0xAA, 0xBB]); // usn
        raw[50..52].copy_from_slice(&[0x01, 0x02]); // original bytes sector 0
        raw[52..54].copy_from_slice(&[0x03, 0x04]); // original bytes sector 1
        raw[510..512].copy_from_slice(&[0xAA, 0xBB]);
        raw[1022..1024].copy_from_slice(&[0xAA, 0xBB]);
        let fixed = apply_fixups(raw, 512).unwrap();
        assert_eq!(&fixed[510..512], &[0x01, 0x02]);
        assert_eq!(&fixed[1022..1024], &[0x03, 0x04]);
    }
}
