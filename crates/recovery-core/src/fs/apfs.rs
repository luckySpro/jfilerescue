//! Read-only APFS parser for deleted-file discovery.
//!
//! Strategy: (1) locate the newest container superblock in the checkpoint
//! area, (2) walk the current volume's file-system B-tree through the object
//! maps to learn which inodes are *live*, then (3) scan every block for
//! orphaned file-system B-tree leaf nodes left behind by copy-on-write and
//! collect inode / extent / directory records whose inode is no longer live.
//!
//! Encrypted (FileVault) volumes are detected and reported as unsupported for
//! this path; their metadata is ciphertext at the device level.

use super::{AllocBitmap, MetadataScan, Volume};
use crate::device::BlockDevice;
use crate::model::{Extent, FoundFile, FsKind, Quality, ScanMode, ScanRequest, Source};
use crate::progress::{Phase, SharedProgress};
use crate::util::{ext_of, le_u16, le_u32, le_u64, name_looks_damaged, normalize_path};
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const OBJ_TYPE_MASK: u32 = 0x0000_FFFF;
const OBJ_NX_SUPERBLOCK: u32 = 1;
const OBJ_BTREE: u32 = 2;
const OBJ_BTREE_NODE: u32 = 3;
const OBJ_FS: u32 = 0xD;
const SUBTYPE_FSTREE: u32 = 0xE;
const BTNODE_ROOT: u16 = 1;
const BTNODE_LEAF: u16 = 2;
const BTNODE_FIXED_KV: u16 = 4;

const J_INODE: u8 = 3;
const J_FILE_EXTENT: u8 = 8;
const J_DIR_REC: u8 = 9;
const ROOT_DIR_INO: u64 = 2;
const PRIV_DIR_INO: u64 = 3;
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const APFS_FS_UNENCRYPTED: u64 = 1;

pub struct ApfsVolume {
    dev: Arc<dyn BlockDevice>,
    block: u64,
    blocks: u64,
    nxsb: Vec<u8>,
    label: Option<String>,
    encrypted: bool,
    /// Physical block of the chosen volume superblock.
    apsb: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Inode {
    id: u64,
    parent: u64,
    private_id: u64,
    mode: u16,
    size: u64,
    name: Option<String>,
    ctime: u64,
    mtime: u64,
    xid: u64,
}

#[derive(Debug, Clone)]
struct FileExtent {
    logical: u64,
    len: u64,
    phys: u64,
    xid: u64,
}

#[derive(Default)]
struct Records {
    inodes: HashMap<u64, Inode>,
    extents: HashMap<u64, Vec<FileExtent>>, // keyed by private_id (dstream)
    /// parent -> (name, child id, xid)
    direntries: HashMap<u64, (u64, String, u64)>, // child -> (parent, name, xid)
}

pub fn fletcher64(data: &[u8]) -> u64 {
    let mut s1: u64 = 0;
    let mut s2: u64 = 0;
    for c in data.as_chunks::<4>().0 {
        s1 = (s1 + le_u32(c, 0) as u64) % 0xFFFF_FFFF;
        s2 = (s2 + s1) % 0xFFFF_FFFF;
    }
    let c1 = 0xFFFF_FFFF - ((s1 + s2) % 0xFFFF_FFFF);
    let c2 = 0xFFFF_FFFF - ((s1 + c1) % 0xFFFF_FFFF);
    (c2 << 32) | c1
}

fn checksum_ok(b: &[u8]) -> bool {
    b.len() >= 32 && le_u64(b, 0) == fletcher64(&b[8..])
}

impl ApfsVolume {
    pub fn open(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        let b0 = dev.read_vec(0, 4096)?;
        if &b0[32..36] != b"NXSB" {
            return Err(Error::UnknownFilesystem);
        }
        let block = le_u32(&b0, 36) as u64;
        if !matches!(block, 4096 | 8192 | 16384 | 32768 | 65536) {
            return Err(Error::Corrupt("APFS 块大小无效".into()));
        }
        let blocks = le_u64(&b0, 40);
        let nxsb = dev.read_vec(0, block as usize)?;
        let mut vol = Self {
            dev,
            block,
            blocks,
            nxsb: nxsb.clone(),
            label: None,
            encrypted: false,
            apsb: vec![],
        };
        vol.nxsb = vol.newest_nxsb().unwrap_or(nxsb);
        // First volume listed in nx_fs_oid[] — the data volume on a single-volume container.
        // For containers with several volumes (System/Data) prefer the one with role Data (0x40) / largest file count.
        vol.apsb = vol.pick_volume()?;
        vol.encrypted = le_u64(&vol.apsb, 264) & APFS_FS_UNENCRYPTED == 0;
        let name: String = String::from_utf8_lossy(&vol.apsb[704..960])
            .trim_end_matches('\0')
            .to_string();
        vol.label = if name.is_empty() { None } else { Some(name) };
        Ok(vol)
    }

    fn read_block(&self, n: u64) -> Result<Vec<u8>> {
        if n >= self.blocks {
            return Err(Error::Corrupt(format!("块 {} 超出容器范围", n)));
        }
        self.dev.read_vec(n * self.block, self.block as usize)
    }

    fn newest_nxsb(&self) -> Option<Vec<u8>> {
        let desc_blocks = (le_u32(&self.nxsb, 104) & 0x7FFF_FFFF) as u64;
        let desc_base = le_u64(&self.nxsb, 112);
        let mut best: Option<(u64, Vec<u8>)> = None;
        for i in 0..desc_blocks.min(4096) {
            let Ok(b) = self.read_block(desc_base + i) else {
                continue;
            };
            if le_u32(&b, 24) & OBJ_TYPE_MASK != OBJ_NX_SUPERBLOCK
                || &b[32..36] != b"NXSB"
                || !checksum_ok(&b)
            {
                continue;
            }
            let xid = le_u64(&b, 16);
            if best.as_ref().map(|(x, _)| xid > *x).unwrap_or(true) {
                best = Some((xid, b));
            }
        }
        best.map(|(_, b)| b)
    }

    fn pick_volume(&self) -> Result<Vec<u8>> {
        let omap_oid = le_u64(&self.nxsb, 160);
        let max_fs = le_u32(&self.nxsb, 180).min(100) as usize;
        let mut best: Option<(u64, Vec<u8>)> = None;
        for i in 0..max_fs {
            let oid = le_u64(&self.nxsb, 184 + i * 8);
            if oid == 0 {
                continue;
            }
            let Some(paddr) = self.omap_lookup(omap_oid, oid) else {
                continue;
            };
            let Ok(b) = self.read_block(paddr) else {
                continue;
            };
            if &b[32..36] != b"APSB" || le_u32(&b, 24) & OBJ_TYPE_MASK != OBJ_FS {
                continue;
            }
            let role = le_u16(&b, 964);
            let files = le_u64(&b, 184);
            // Prefer the Data role; otherwise most files.
            let score = if role == 0x40 { u64::MAX } else { files };
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, b));
            }
        }
        best.map(|(_, b)| b)
            .ok_or_else(|| Error::Corrupt("未找到 APFS 卷超级块".into()))
    }

    /// Look up a virtual oid in an object map (physical omap block). Returns
    /// the physical address of the newest mapping.
    fn omap_lookup(&self, omap_paddr: u64, oid: u64) -> Option<u64> {
        let om = self.read_block(omap_paddr).ok()?;
        let tree = le_u64(&om, 48);
        let mut node = self.read_block(tree).ok()?;
        for _ in 0..16 {
            let n = parse_node(&node, self.block as usize)?;
            // Keys: {oid u64, xid u64}; find last key with key.oid == oid (max xid).
            let mut chosen: Option<usize> = None;
            for (i, (k, _)) in n.entries.iter().enumerate() {
                if k.len() < 16 {
                    continue;
                }
                let koid = le_u64(k, 0);
                if koid <= oid {
                    chosen = Some(i);
                }
                if koid > oid {
                    break;
                }
            }
            let idx = chosen?;
            let (k, v) = &n.entries[idx];
            if n.leaf {
                if le_u64(k, 0) != oid || v.len() < 16 {
                    return None;
                }
                return Some(le_u64(v, 8));
            }
            if v.len() < 8 {
                return None;
            }
            node = self.read_block(le_u64(v, 0)).ok()?;
        }
        None
    }

    /// Walk the live fs tree, collecting records.
    fn walk_live(&self, progress: &SharedProgress) -> Result<(Records, Vec<u64>)> {
        let vol_omap = le_u64(&self.apsb, 128);
        let root_oid = le_u64(&self.apsb, 136);
        let root = self
            .omap_lookup(vol_omap, root_oid)
            .ok_or_else(|| Error::Corrupt("无法解析卷对象映射".into()))?;
        let mut recs = Records::default();
        let mut meta_blocks = vec![];
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(p) = stack.pop() {
            progress.checkpoint()?;
            if !seen.insert(p) || seen.len() > 4_000_000 {
                continue;
            }
            meta_blocks.push(p);
            let Ok(b) = self.read_block(p) else { continue };
            let Some(n) = parse_node(&b, self.block as usize) else {
                continue;
            };
            let xid = le_u64(&b, 16);
            if n.leaf {
                for (k, v) in &n.entries {
                    collect_record(&mut recs, k, v, xid);
                }
            } else {
                for (_, v) in &n.entries {
                    if v.len() >= 8 {
                        if let Some(child) = self.omap_lookup(vol_omap, le_u64(v, 0)) {
                            stack.push(child);
                        }
                    }
                }
            }
        }
        Ok((recs, meta_blocks))
    }

    /// Scan all blocks for orphaned fs-tree leaf nodes.
    fn scan_orphans(
        &self,
        live_nodes: &HashSet<u64>,
        progress: &SharedProgress,
    ) -> Result<Records> {
        let mut recs = Records::default();
        let chunk_blocks = (1u64 << 20) / self.block;
        progress.set_phase(Phase::Metadata, self.blocks);
        progress.set_message("扫描 APFS 残留节点");
        let mut buf = vec![0u8; (chunk_blocks * self.block) as usize];
        let mut n = 0u64;
        while n < self.blocks {
            progress.checkpoint()?;
            let cnt = chunk_blocks.min(self.blocks - n);
            let bytes = (cnt * self.block) as usize;
            let got = self.dev.read_lossy(n * self.block, &mut buf[..bytes]);
            if got < bytes {
                progress.add_read_error();
            }
            progress.add_bytes(bytes as u64);
            for i in 0..cnt {
                let blk = n + i;
                if live_nodes.contains(&blk) {
                    continue;
                }
                let b = &buf[(i * self.block) as usize..((i + 1) * self.block) as usize];
                let t = le_u32(b, 24) & OBJ_TYPE_MASK;
                if (t != OBJ_BTREE_NODE && t != OBJ_BTREE) || le_u32(b, 28) != SUBTYPE_FSTREE {
                    continue;
                }
                if !checksum_ok(b) {
                    continue;
                }
                let Some(node) = parse_node(b, self.block as usize) else {
                    continue;
                };
                if !node.leaf {
                    continue;
                }
                let xid = le_u64(b, 16);
                for (k, v) in &node.entries {
                    collect_record(&mut recs, k, v, xid);
                }
            }
            n += cnt;
            progress.set_done(n);
        }
        Ok(recs)
    }
}

struct Node {
    leaf: bool,
    entries: Vec<(Vec<u8>, Vec<u8>)>,
}

fn parse_node(b: &[u8], block: usize) -> Option<Node> {
    if b.len() < 56 {
        return None;
    }
    let flags = le_u16(b, 32);
    let nkeys = le_u32(b, 36) as usize;
    let ts_off = le_u16(b, 40) as usize;
    let ts_len = le_u16(b, 42) as usize;
    let data = 56usize;
    let toc = data + ts_off;
    let key_area = toc + ts_len;
    let val_end = if flags & BTNODE_ROOT != 0 {
        block - 40
    } else {
        block
    };
    if nkeys > 4096 || key_area > b.len() {
        return None;
    }
    let fixed = flags & BTNODE_FIXED_KV != 0;
    let esz = if fixed { 4 } else { 8 };
    if toc + nkeys * esz > key_area {
        return None;
    }
    let mut entries = Vec::with_capacity(nkeys);
    for i in 0..nkeys {
        let e = toc + i * esz;
        let (ko, kl, vo, vl) = if fixed {
            // Fixed sizes: omap keys 16/vals 16 (leaf) or 8 (index). We infer by node type later;
            // here we read conservatively: key 16, value 16 leaf / 8 index.
            let ko = le_u16(b, e) as usize;
            let vo = le_u16(b, e + 2) as usize;
            let vl = if flags & BTNODE_LEAF != 0 { 16 } else { 8 };
            (ko, 16usize, vo, vl)
        } else {
            (
                le_u16(b, e) as usize,
                le_u16(b, e + 2) as usize,
                le_u16(b, e + 4) as usize,
                le_u16(b, e + 6) as usize,
            )
        };
        let ks = key_area + ko;
        if ks + kl > b.len() || vo > val_end || vo < vl {
            return None;
        }
        let vs = val_end - vo;
        entries.push((b[ks..ks + kl].to_vec(), b[vs..vs + vl].to_vec()));
    }
    Some(Node {
        leaf: flags & BTNODE_LEAF != 0,
        entries,
    })
}

fn collect_record(r: &mut Records, k: &[u8], v: &[u8], xid: u64) {
    if k.len() < 8 {
        return;
    }
    let hdr = le_u64(k, 0);
    let id = hdr & 0x0FFF_FFFF_FFFF_FFFF;
    let t = (hdr >> 60) as u8;
    match t {
        J_INODE if v.len() >= 92 => {
            let mode = le_u16(v, 80);
            let mut name = None;
            let mut size = le_u64(v, 84);
            // xfields
            if v.len() > 96 {
                let nx = le_u16(v, 92) as usize;
                let mut hdrp = 96;
                let mut datap = 96 + nx * 4;
                datap = (datap + 7) & !7;
                for _ in 0..nx {
                    if hdrp + 4 > v.len() {
                        break;
                    }
                    let xt = v[hdrp];
                    let xs = le_u16(v, hdrp + 2) as usize;
                    if datap + xs > v.len() {
                        break;
                    }
                    match xt {
                        4 => {
                            name = Some(
                                String::from_utf8_lossy(&v[datap..datap + xs])
                                    .trim_end_matches('\0')
                                    .to_string(),
                            )
                        }
                        8 if xs >= 8 => size = le_u64(v, datap),
                        _ => {}
                    }
                    hdrp += 4;
                    datap += (xs + 7) & !7;
                }
            }
            let ino = Inode {
                id,
                parent: le_u64(v, 0),
                private_id: le_u64(v, 8),
                mode,
                size,
                name,
                ctime: le_u64(v, 16),
                mtime: le_u64(v, 24),
                xid,
            };
            match r.inodes.get(&id) {
                Some(e) if e.xid >= xid => {}
                _ => {
                    r.inodes.insert(id, ino);
                }
            }
        }
        J_FILE_EXTENT if k.len() >= 16 && v.len() >= 16 => {
            let logical = le_u64(k, 8);
            let len = le_u64(v, 0) & 0x00FF_FFFF_FFFF_FFFF;
            let phys = le_u64(v, 8);
            let list = r.extents.entry(id).or_default();
            if let Some(e) = list.iter_mut().find(|e| e.logical == logical) {
                if xid > e.xid {
                    *e = FileExtent {
                        logical,
                        len,
                        phys,
                        xid,
                    };
                }
            } else {
                list.push(FileExtent {
                    logical,
                    len,
                    phys,
                    xid,
                });
            }
        }
        J_DIR_REC if k.len() > 12 && v.len() >= 8 => {
            // hashed key: u32 len|hash then name; unhashed: u16 len then name.
            let (name, ok) = {
                let l = (le_u32(k, 8) & 0x3FF) as usize;
                if 12 + l == k.len() && l > 0 {
                    (
                        String::from_utf8_lossy(&k[12..12 + l])
                            .trim_end_matches('\0')
                            .to_string(),
                        true,
                    )
                } else {
                    let l2 = le_u16(k, 8) as usize;
                    if 10 + l2 == k.len() && l2 > 0 {
                        (
                            String::from_utf8_lossy(&k[10..10 + l2])
                                .trim_end_matches('\0')
                                .to_string(),
                            true,
                        )
                    } else {
                        (String::new(), false)
                    }
                }
            };
            if ok {
                let child = le_u64(v, 0);
                match r.direntries.get(&child) {
                    Some((_, _, x)) if *x >= xid => {}
                    _ => {
                        r.direntries.insert(child, (id, name, xid));
                    }
                }
            }
        }
        _ => {}
    }
}

impl Volume for ApfsVolume {
    fn kind(&self) -> FsKind {
        FsKind::Apfs
    }
    fn label(&self) -> Option<String> {
        self.label.clone()
    }
    fn cluster_size(&self) -> u64 {
        self.block
    }
    fn total_clusters(&self) -> u64 {
        self.blocks
    }

    fn scan_metadata(&self, req: &ScanRequest, progress: &SharedProgress) -> Result<MetadataScan> {
        if self.encrypted {
            return Err(Error::Unsupported(
                "APFS 卷已启用 FileVault 加密，设备层读取为密文".into(),
            ));
        }
        progress.set_phase(Phase::Metadata, 0);
        progress.set_message("遍历当前文件系统树");
        let (live, live_blocks) = self.walk_live(progress)?;
        let live_set: HashSet<u64> = live_blocks.iter().copied().collect();

        // Allocation approximation: live file extents + metadata nodes.
        let mut bitmap = AllocBitmap::new(self.block, self.blocks);
        for b in &live_blocks {
            bitmap.set(*b, true);
        }
        for list in live.extents.values() {
            for e in list {
                let nb = e.len.div_ceil(self.block);
                for i in 0..nb {
                    bitmap.set(e.phys + i, true);
                }
            }
        }
        // Reserve the checkpoint/descriptor areas and the first blocks.
        for b in 0..64.min(self.blocks) {
            bitmap.set(b, true);
        }

        let orphans = self.scan_orphans(&live_set, progress)?;
        progress.set_message("重建目录结构");

        // Name/parent resolution: live records first, then orphans.
        let name_of = |id: u64| -> Option<(u64, String)> {
            live.direntries
                .get(&id)
                .map(|(p, n, _)| (*p, n.clone()))
                .or_else(|| orphans.direntries.get(&id).map(|(p, n, _)| (*p, n.clone())))
                .or_else(|| {
                    live.inodes
                        .get(&id)
                        .or_else(|| orphans.inodes.get(&id))
                        .and_then(|i| i.name.clone().map(|n| (i.parent, n)))
                })
        };
        let mut path_cache: HashMap<u64, Option<String>> = HashMap::new();
        fn path_of(
            id: u64,
            name_of: &dyn Fn(u64) -> Option<(u64, String)>,
            cache: &mut HashMap<u64, Option<String>>,
            depth: u32,
        ) -> Option<String> {
            if id == ROOT_DIR_INO {
                return Some(String::new());
            }
            if id == PRIV_DIR_INO || depth > 64 {
                return None;
            }
            if let Some(c) = cache.get(&id) {
                return c.clone();
            }
            cache.insert(id, None);
            let r = name_of(id).and_then(|(p, n)| {
                path_of(p, name_of, cache, depth + 1).map(|pp| {
                    if pp.is_empty() {
                        n
                    } else {
                        format!("{}/{}", pp, n)
                    }
                })
            });
            cache.insert(id, r.clone());
            r
        }

        let target = req.directory.as_deref().map(normalize_path);
        let mut files = vec![];
        let mut id_counter = 1u64;
        for (ino_id, ino) in orphans.inodes.iter() {
            progress.checkpoint()?;
            if live.inodes.contains_key(ino_id) {
                continue; // still exists
            }
            if ino.mode & S_IFMT != S_IFREG {
                continue;
            }
            let (dir, name, damaged) = match name_of(*ino_id) {
                Some((parent, n)) => {
                    let d = path_of(parent, &name_of, &mut path_cache, 0);
                    let dmg = name_looks_damaged(&n);
                    (d, if dmg { format!("INODE_{}", ino_id) } else { n }, dmg)
                }
                None => (None, format!("INODE_{}", ino_id), true),
            };
            if damaged && !req.include_damaged_names {
                continue;
            }
            if req.mode == ScanMode::Directory {
                let Some(t) = &target else { continue };
                if !crate::model::path_has_prefix(dir.as_deref().unwrap_or(""), t) {
                    continue;
                }
            }
            let ext = ext_of(&name);
            if !req.extensions.is_empty()
                && !req.extensions.iter().any(|x| x.eq_ignore_ascii_case(&ext))
            {
                continue;
            }
            let mut exts: Vec<FileExtent> = orphans
                .extents
                .get(&ino.private_id)
                .cloned()
                .unwrap_or_default();
            exts.sort_by_key(|e| e.logical);
            let mut remaining = ino.size;
            let mut extents = vec![];
            let mut covered = 0u64;
            for e in exts {
                if remaining == 0 {
                    break;
                }
                if e.logical != covered {
                    break; // gap → truncated
                }
                let l = e.len.min(remaining);
                if e.phys != 0 {
                    extents.push(Extent {
                        offset: e.phys * self.block,
                        len: l,
                    });
                }
                covered += e.len;
                remaining -= l;
            }
            let in_range = extents.iter().all(|x| x.offset + x.len <= self.dev.size());
            let complete = remaining == 0 && ino.size > 0;
            let ov = bitmap.overwritten_pct(&extents);
            let (quality, note) = if !in_range {
                (Quality::Low, "数据块超出卷范围".into())
            } else if ino.size == 0 || extents.is_empty() {
                (Quality::Low, "没有可用的数据块记录".into())
            } else if !complete {
                (
                    Quality::Low,
                    format!("仅找到 {}% 的数据块", covered * 100 / ino.size.max(1)),
                )
            } else if ov == 0 {
                (Quality::High, "数据块未被当前文件占用".into())
            } else if ov <= 30 {
                (Quality::Medium, format!("{}% 数据块已被重新使用", ov))
            } else {
                (Quality::Low, format!("{}% 数据块已被重新使用", ov))
            };
            files.push(FoundFile {
                id: id_counter,
                name,
                dir,
                size: ino.size,
                source: Source::Metadata,
                quality,
                overwritten_pct: ov,
                deleted_at: None,
                modified_at: Some((ino.mtime / 1_000_000_000) as i64),
                created_at: Some((ino.ctime / 1_000_000_000) as i64),
                ext,
                extents,
                resident: None,
                name_damaged: damaged,
                note,
            });
            id_counter += 1;
            progress.add_found(1);
        }
        Ok(MetadataScan {
            files,
            bitmap,
            volume_label: self.label.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fletcher_zero_block_roundtrip() {
        let mut b = vec![0u8; 4096];
        b[40] = 7;
        let c = fletcher64(&b[8..]);
        b[0..8].copy_from_slice(&c.to_le_bytes());
        assert!(checksum_ok(&b));
        b[100] ^= 1;
        assert!(!checksum_ok(&b));
    }
}
