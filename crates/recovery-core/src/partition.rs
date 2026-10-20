//! MBR / GPT partition table parsing (read-only). Also detects a bare
//! file system at offset 0 (super-floppy / volume handle).

use crate::device::BlockDevice;
use crate::fs::detect_fs;
use crate::model::FsKind;
use crate::util::{le_u16, le_u32, le_u64};
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionInfo {
    pub index: u32,
    pub start: u64,
    pub len: u64,
    pub fs: FsKind,
    pub label: Option<String>,
    pub type_desc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    Gpt,
    Mbr,
    /// No table; file system starts at byte 0.
    None,
    /// Nothing recognizable at all.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionTable {
    pub kind: TableKind,
    pub partitions: Vec<PartitionInfo>,
}

const GPT_SIG: &[u8; 8] = b"EFI PART";

pub fn read_partitions<D: BlockDevice>(dev: &D) -> Result<PartitionTable> {
    let ss = dev.sector_size() as u64;
    let size = dev.size();
    if size < ss * 2 {
        return Ok(PartitionTable {
            kind: TableKind::Unknown,
            partitions: vec![],
        });
    }

    // GPT header lives in LBA 1.
    let mut hdr = vec![0u8; ss as usize];
    if dev.read_at(ss, &mut hdr).is_ok() && &hdr[0..8] == GPT_SIG {
        if let Some(t) = parse_gpt(dev, &hdr) {
            return Ok(t);
        }
    }

    let mut lba0 = vec![0u8; ss as usize];
    dev.read_at(0, &mut lba0)?;

    if lba0[510] == 0x55 && lba0[511] == 0xAA {
        let parts = parse_mbr(dev, &lba0);
        // An MBR with no partitions but a valid FAT/NTFS boot sector at 0 is a volume.
        if !parts.is_empty() {
            return Ok(PartitionTable {
                kind: TableKind::Mbr,
                partitions: parts,
            });
        }
    }

    // Bare file system?
    let fs = detect_fs(dev, 0);
    if fs != FsKind::Unknown {
        return Ok(PartitionTable {
            kind: TableKind::None,
            partitions: vec![PartitionInfo {
                index: 0,
                start: 0,
                len: size,
                fs,
                label: None,
                type_desc: "整卷".into(),
            }],
        });
    }
    Ok(PartitionTable {
        kind: TableKind::Unknown,
        partitions: vec![],
    })
}

fn parse_gpt<D: BlockDevice>(dev: &D, hdr: &[u8]) -> Option<PartitionTable> {
    let ss = dev.sector_size() as u64;
    let entries_lba = le_u64(hdr, 72);
    let n = le_u32(hdr, 80) as u64;
    let esize = le_u32(hdr, 84) as u64;
    if esize < 128 || n == 0 || n > 1024 {
        return None;
    }
    let total = n * esize;
    let buf = dev.read_vec(entries_lba * ss, total as usize).ok()?;
    let mut parts = vec![];
    for i in 0..n as usize {
        let e = &buf[i * esize as usize..(i + 1) * esize as usize];
        let type_guid = &e[0..16];
        if type_guid.iter().all(|b| *b == 0) {
            continue;
        }
        let first = le_u64(e, 32);
        let last = le_u64(e, 40);
        if last < first {
            continue;
        }
        let start = first * ss;
        let len = (last - first + 1) * ss;
        if start + len > dev.size() {
            continue;
        }
        let name = crate::util::utf16le(&e[56..128])
            .trim_end_matches('\0')
            .to_string();
        let fs = detect_fs(dev, start);
        parts.push(PartitionInfo {
            index: i as u32,
            start,
            len,
            fs,
            label: if name.is_empty() { None } else { Some(name) },
            type_desc: gpt_type_desc(type_guid).to_string(),
        });
    }
    Some(PartitionTable {
        kind: TableKind::Gpt,
        partitions: parts,
    })
}

fn gpt_type_desc(g: &[u8]) -> &'static str {
    // GUIDs are mixed-endian; compare against known byte layouts.
    const BASIC_DATA: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];
    const EFI_SYSTEM: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    const MS_RESERVED: [u8; 16] = [
        0x16, 0xE3, 0xC9, 0xE3, 0x5C, 0x0B, 0xB8, 0x4D, 0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15,
        0xAE,
    ];
    const APFS: [u8; 16] = [
        0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC,
        0xAC,
    ];
    const WIN_RECOVERY: [u8; 16] = [
        0xA4, 0xBB, 0x94, 0xDE, 0xD1, 0x06, 0x40, 0x4D, 0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6,
        0xAC,
    ];
    match g {
        x if x == BASIC_DATA => "基本数据分区",
        x if x == EFI_SYSTEM => "EFI 系统分区",
        x if x == MS_RESERVED => "Microsoft 保留分区",
        x if x == APFS => "Apple APFS 容器",
        x if x == WIN_RECOVERY => "Windows 恢复分区",
        _ => "其他",
    }
}

fn parse_mbr<D: BlockDevice>(dev: &D, lba0: &[u8]) -> Vec<PartitionInfo> {
    let ss = dev.sector_size() as u64;
    let mut parts = vec![];
    for i in 0..4 {
        let e = &lba0[446 + i * 16..446 + (i + 1) * 16];
        let ptype = e[4];
        let first = le_u32(e, 8) as u64;
        let count = le_u32(e, 12) as u64;
        if ptype == 0 || count == 0 {
            continue;
        }
        let start = first * ss;
        let len = count * ss;
        if start + len > dev.size() {
            continue;
        }
        // Extended partition: walk the EBR chain.
        if matches!(ptype, 0x05 | 0x0F | 0x85) {
            parts.extend(walk_ebr(dev, start, len, parts.len() as u32 + 4));
            continue;
        }
        let fs = detect_fs(dev, start);
        parts.push(PartitionInfo {
            index: i as u32,
            start,
            len,
            fs,
            label: None,
            type_desc: mbr_type_desc(ptype),
        });
    }
    parts
}

fn walk_ebr<D: BlockDevice>(
    dev: &D,
    ext_start: u64,
    ext_len: u64,
    mut idx: u32,
) -> Vec<PartitionInfo> {
    let ss = dev.sector_size() as u64;
    let mut out = vec![];
    let mut cur = ext_start;
    let mut guard = 0;
    while guard < 128 {
        guard += 1;
        let Ok(sec) = dev.read_vec(cur, ss as usize) else {
            break;
        };
        if sec[510] != 0x55 || sec[511] != 0xAA {
            break;
        }
        let e1 = &sec[446..462];
        let e2 = &sec[462..478];
        let t1 = e1[4];
        if t1 != 0 {
            let start = cur + le_u32(e1, 8) as u64 * ss;
            let len = le_u32(e1, 12) as u64 * ss;
            if start + len <= dev.size() {
                let fs = detect_fs(dev, start);
                out.push(PartitionInfo {
                    index: idx,
                    start,
                    len,
                    fs,
                    label: None,
                    type_desc: mbr_type_desc(t1),
                });
                idx += 1;
            }
        }
        if e2[4] == 0 {
            break;
        }
        let next = ext_start + le_u32(e2, 8) as u64 * ss;
        if next <= cur || next >= ext_start + ext_len {
            break;
        }
        cur = next;
    }
    out
}

fn mbr_type_desc(t: u8) -> String {
    match t {
        0x07 => "NTFS/exFAT".into(),
        0x0B | 0x0C => "FAT32".into(),
        0x04 | 0x06 | 0x0E => "FAT16".into(),
        0x01 => "FAT12".into(),
        0xAF => "HFS/HFS+".into(),
        0xEE => "GPT 保护分区".into(),
        0x83 => "Linux".into(),
        0x82 => "Linux swap".into(),
        _ => format!("类型 0x{:02X}", t),
    }
}

#[allow(dead_code)]
fn _unused(b: &[u8]) -> u16 {
    le_u16(b, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;

    #[test]
    fn mbr_single_partition() {
        let mut img = vec![0u8; 512 * 64];
        img[510] = 0x55;
        img[511] = 0xAA;
        let e = 446;
        img[e + 4] = 0x07;
        img[e + 8..e + 12].copy_from_slice(&8u32.to_le_bytes());
        img[e + 12..e + 16].copy_from_slice(&32u32.to_le_bytes());
        let t = read_partitions(&MemDevice::new(img)).unwrap();
        assert_eq!(t.kind, TableKind::Mbr);
        assert_eq!(t.partitions.len(), 1);
        assert_eq!(t.partitions[0].start, 8 * 512);
        assert_eq!(t.partitions[0].len, 32 * 512);
    }

    #[test]
    fn empty_disk() {
        let t = read_partitions(&MemDevice::new(vec![0u8; 4096])).unwrap();
        assert_eq!(t.kind, TableKind::Unknown);
    }
}
