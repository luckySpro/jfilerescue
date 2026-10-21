//! Integration tests against real file system images.
//!
//! * FAT32: built in-process with the `fatfs` crate (no OS privileges needed).
//! * NTFS / exFAT / APFS: read from `test-images/*.img` produced by
//!   `scripts/make-test-images.mjs` on the host OS; skipped when absent.

use recovery_core::carve::signatures::SignatureLibrary;
use recovery_core::device::{BlockDevice, FileDevice, MemDevice};
use recovery_core::model::{Quality, ScanDepth, ScanMode, ScanRequest, Source};
use recovery_core::progress::Progress;
use recovery_core::recover::{read_file_data, recover, Conflict, RecoverOptions};
use recovery_core::scan::run_scan;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn payload_jpg() -> Vec<u8> {
    // SOI, APP0 (len 16), SOS (len 2), entropy data, EOI — a structurally valid skeleton.
    let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    v.extend_from_slice(b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00");
    v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
    v.extend(std::iter::repeat_n(0x37, 200_000));
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

/// Build a 64 MiB FAT32 volume, write files, delete some, return the image.
fn build_fat32() -> Vec<u8> {
    let size = 64 << 20;
    let mut img = Cursor::new(vec![0u8; size]);
    fatfs::format_volume(
        &mut img,
        fatfs::FormatVolumeOptions::new()
            .fat_type(fatfs::FatType::Fat32)
            .volume_label(*b"JFRTEST    "),
    )
    .unwrap();
    {
        let fs = fatfs::FileSystem::new(&mut img, fatfs::FsOptions::new()).unwrap();
        let root = fs.root_dir();
        let proj = root
            .create_dir("项目")
            .unwrap()
            .create_dir("2025-合同")
            .unwrap();
        let att = proj.create_dir("附件").unwrap();
        proj.create_file("合同-华东区-2025Q2.docx")
            .unwrap()
            .write_all(&[0x55; 50_000])
            .unwrap();
        att.create_file("报价单-v3.xlsx")
            .unwrap()
            .write_all(&[0x66; 30_000])
            .unwrap();
        proj.create_file("IMG_0001.jpg")
            .unwrap()
            .write_all(&payload_jpg())
            .unwrap();
        let other = root.create_dir("其他").unwrap();
        other
            .create_file("keep.txt")
            .unwrap()
            .write_all(b"this stays")
            .unwrap();
        other
            .create_file("gone.txt")
            .unwrap()
            .write_all(&[b'y'; 5000])
            .unwrap();
        proj.remove("合同-华东区-2025Q2.docx").unwrap();
        att.remove("报价单-v3.xlsx").unwrap();
        proj.remove("IMG_0001.jpg").unwrap();
        other.remove("gone.txt").unwrap();
    }
    img.into_inner()
}

#[test]
fn fat32_directory_recovery_roundtrip() {
    let img = build_fat32();
    // Also persist the generated image so the CLI can be smoke-tested against it.
    let gen = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fat32-gen.img");
    let _ = std::fs::write(&gen, &img);
    let dev: Arc<dyn BlockDevice> = Arc::new(MemDevice::new(img));
    let sigs = SignatureLibrary::builtin();
    let p = Progress::new();

    // Directory mode: only files under 项目/2025-合同.
    let req = ScanRequest {
        mode: ScanMode::Directory,
        depth: ScanDepth::Quick,
        directory: Some("项目/2025-合同".into()),
        ..Default::default()
    };
    let out = run_scan(Arc::clone(&dev), None, &req, &sigs, &p).unwrap();
    let names: Vec<String> = out.files.iter().map(|f| f.full_path()).collect();
    assert!(
        names.contains(&"项目/2025-合同/合同-华东区-2025Q2.docx".to_string()),
        "{:?}",
        names
    );
    assert!(
        names.contains(&"项目/2025-合同/附件/报价单-v3.xlsx".to_string()),
        "{:?}",
        names
    );
    assert!(
        names.contains(&"项目/2025-合同/IMG_0001.jpg".to_string()),
        "{:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.contains("gone.txt")),
        "directory filter leaked: {:?}",
        names
    );
    assert!(!names.iter().any(|n| n.contains("keep.txt")));

    // Data integrity.
    let jpg = out.files.iter().find(|f| f.name == "IMG_0001.jpg").unwrap();
    assert_eq!(jpg.size, payload_jpg().len() as u64);
    assert!(jpg.quality >= Quality::Medium, "{:?}", jpg);
    let mut got = vec![];
    read_file_data(dev.as_ref(), jpg, |c| {
        got.extend_from_slice(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(sha(&got), sha(&payload_jpg()));

    // Recover to a temp dir on the local disk (source is an in-memory device → never same disk).
    let tmp = tempfile::tempdir().unwrap();
    let opts = RecoverOptions {
        dest: tmp.path().join("out"),
        keep_structure: true,
        conflict: Conflict::Rename,
        verify: true,
        restore_times: false,
    };
    let rep = recover(
        dev.as_ref(),
        "mem://fat32",
        &out.files,
        &opts,
        &Progress::new(),
    )
    .unwrap();
    assert_eq!(rep.failed, 0, "{:?}", rep.items);
    let recovered = std::fs::read(tmp.path().join("out/项目/2025-合同/IMG_0001.jpg")).unwrap();
    assert_eq!(sha(&recovered), sha(&payload_jpg()));
    assert_eq!(
        std::fs::read(tmp.path().join("out/项目/2025-合同/附件/报价单-v3.xlsx"))
            .unwrap()
            .len(),
        30_000
    );

    // Full + deep: carving on the free clusters must also find the JPEG by signature.
    let req = ScanRequest {
        mode: ScanMode::Full,
        depth: ScanDepth::Deep,
        extensions: vec!["jpg".into()],
        ..Default::default()
    };
    let out = run_scan(Arc::clone(&dev), None, &req, &sigs, &Progress::new()).unwrap();
    let carved: Vec<_> = out
        .files
        .iter()
        .filter(|f| f.source == Source::Carved)
        .collect();
    assert!(!carved.is_empty(), "no carved jpg");
    assert!(
        carved.iter().any(|f| f.size == payload_jpg().len() as u64),
        "carved sizes: {:?}",
        carved.iter().map(|f| f.size).collect::<Vec<_>>()
    );
}

fn host_image(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-images")
        .join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!(
            "skipped: {} not present (run scripts/make-test-images.mjs)",
            p.display()
        );
        None
    }
}

fn check_host_image(name: &str, expect_fs: &str) {
    let Some(p) = host_image(name) else { return };
    let dev: Arc<dyn BlockDevice> = Arc::new(FileDevice::open(&p).unwrap());
    let sigs = SignatureLibrary::builtin();
    let req = ScanRequest {
        mode: ScanMode::Directory,
        depth: ScanDepth::Quick,
        directory: Some("项目/2025-合同".into()),
        ..Default::default()
    };
    let out = run_scan(Arc::clone(&dev), None, &req, &sigs, &Progress::new()).unwrap();
    assert_eq!(out.fs.label(), expect_fs, "warning: {:?}", out.warning);
    // diskpart images carry an MBR: extents are relative to the scanned volume.
    let vol = recovery_core::device::SliceDevice::new(
        Arc::clone(&dev),
        out.volume_offset,
        out.volume_len,
        "vol",
    )
    .unwrap();
    let names: Vec<String> = out.files.iter().map(|f| f.full_path()).collect();
    // FAT 8.3 entries lose their first byte on delete (0xE5); when no LFN was
    // stored (8.3-compatible names) the scanner reports `_MG_0001.jpg`. That loss
    // is inherent to FAT, so accept it there.
    let find = |want: &str| -> Option<&recovery_core::model::FoundFile> {
        out.files.iter().find(|f| {
            let fp = f.full_path();
            if fp.eq_ignore_ascii_case(want) {
                return true;
            }
            if expect_fs != "FAT32" {
                return false;
            }
            let (wd, wn) = want.rsplit_once('/').unwrap_or(("", want));
            let fd = f.dir.as_deref().unwrap_or("");
            fd.eq_ignore_ascii_case(wd)
                && f.name.starts_with('_')
                && f.name
                    .chars()
                    .skip(1)
                    .collect::<String>()
                    .eq_ignore_ascii_case(&wn.chars().skip(1).collect::<String>())
        })
    };
    for want in [
        "项目/2025-合同/合同-华东区-2025Q2.docx",
        "项目/2025-合同/附件/报价单-v3.xlsx",
        "项目/2025-合同/现场照片/IMG_0001.jpg",
        "项目/2025-合同/签字页.pdf",
    ] {
        assert!(find(want).is_some(), "{} missing in {:?}", want, names);
    }
    assert!(
        !names.iter().any(|n| n.contains("gone.txt")),
        "directory filter leaked: {:?}",
        names
    );
    let jpg = find("项目/2025-合同/现场照片/IMG_0001.jpg").unwrap();
    let mut got = vec![];
    read_file_data(&vol, jpg, |c| {
        got.extend_from_slice(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        sha(&got),
        sha(&payload_jpg()),
        "{} data mismatch (quality {:?}, note {})",
        name,
        jpg.quality,
        jpg.note
    );
    assert!(
        jpg.quality >= Quality::Medium && jpg.overwritten_pct == 0,
        "{}: freed clusters must read as free: {:?} {}% {}",
        name,
        jpg.quality,
        jpg.overwritten_pct,
        jpg.note
    );
    if expect_fs == "NTFS" {
        let small = out
            .files
            .iter()
            .find(|f| f.name == "small.txt")
            .expect("resident file");
        assert!(small.resident.is_some(), "small.txt should be resident");
        // Deleted via the Recycle Bin: must be reported under its ORIGINAL path,
        // flagged, with the real deletion time from the $I record; neither the
        // `$I` record nor a raw `$Recycle.Bin/...` path may leak into the list.
        let rec = out
            .files
            .iter()
            .find(|f| f.name == "recycled.pdf")
            .unwrap_or_else(|| {
                panic!(
                    "recycle-bin file not resolved to its original name: {:?}",
                    names
                )
            });
        assert!(rec.via_recycle_bin, "{:?}", rec.note);
        assert_eq!(rec.dir.as_deref(), Some("项目/2025-合同"));
        assert!(
            rec.deleted_at.map(|t| t > 1_700_000_000).unwrap_or(false),
            "deleted_at from $I: {:?}",
            rec.deleted_at
        );
        assert!(
            !names.iter().any(|n| n.contains("$I")),
            "$I records listed: {:?}",
            names
        );
        assert!(
            !names.iter().any(|n| n.starts_with("$Recycle.Bin")),
            "raw $R paths listed: {:?}",
            names
        );
        let mut got = vec![];
        read_file_data(&vol, rec, |c| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert!(
            got.starts_with(b"%PDF-1.4"),
            "recycled.pdf data: {:?}",
            &got[..8.min(got.len())]
        );
        let mut got = vec![];
        read_file_data(&vol, small, |c| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert!(got.starts_with(b"resident-data-"));
    }
    // Full scan also sees the file deleted outside the target dir.
    let req = ScanRequest {
        mode: ScanMode::Full,
        depth: ScanDepth::Quick,
        ..Default::default()
    };
    let out = run_scan(dev, None, &req, &sigs, &Progress::new()).unwrap();
    // FAT 8.3 entries lose their first byte on delete → `_one.txt`.
    assert!(
        out.files
            .iter()
            .any(|f| f.name == "gone.txt" || (expect_fs == "FAT32" && f.name == "_one.txt")),
        "{:?}",
        out.files.iter().map(|f| f.full_path()).collect::<Vec<_>>()
    );
}

#[test]
fn ntfs_host_image() {
    check_host_image("ntfs.img", "NTFS");
}
#[test]
fn fat32_host_image() {
    check_host_image("fat32.img", "FAT32");
}
#[test]
fn exfat_host_image() {
    check_host_image("exfat.img", "exFAT");
}
#[test]
fn apfs_host_image() {
    check_host_image("apfs.img", "APFS");
}

/// Images built while Windows TRIM was active: the metadata survives but every
/// freed cluster was zero-filled by the VHD driver. The scanner must still find
/// the entries, but report them as erased instead of "High".
fn check_trimmed_image(name: &str, expect_fs: &str) {
    let Some(p) = host_image(name) else { return };
    let dev: Arc<dyn BlockDevice> = Arc::new(FileDevice::open(&p).unwrap());
    let req = ScanRequest {
        mode: ScanMode::Full,
        depth: ScanDepth::Quick,
        ..Default::default()
    };
    let out = run_scan(
        dev,
        None,
        &req,
        &SignatureLibrary::builtin(),
        &Progress::new(),
    )
    .unwrap();
    assert_eq!(out.fs.label(), expect_fs);
    let jpg = out
        .files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with("mg_0001.jpg"))
        .unwrap_or_else(|| {
            panic!(
                "IMG_0001.jpg not found in {:?}",
                out.files.iter().map(|f| f.full_path()).collect::<Vec<_>>()
            )
        });
    assert_eq!(jpg.quality, Quality::Low, "{}", jpg.note);
    assert!(
        jpg.note.contains("擦除"),
        "note should explain TRIM: {}",
        jpg.note
    );
    let w = out.warning.clone().unwrap_or_default();
    assert!(
        w.contains("擦除"),
        "outcome warning should mention erasure: {:?}",
        out.warning
    );
    // Resident NTFS data lives in the MFT and survives TRIM.
    if expect_fs == "NTFS" {
        let small = out
            .files
            .iter()
            .find(|f| f.name == "small.txt")
            .expect("small.txt");
        assert!(small.resident.is_some());
        assert_eq!(small.quality, Quality::High);
    }
}

#[test]
fn ntfs_trimmed_image() {
    check_trimmed_image("ntfs-trimmed.img", "NTFS");
}
#[test]
fn fat32_trimmed_image() {
    check_trimmed_image("fat32-trimmed.img", "FAT32");
}
#[test]
fn exfat_trimmed_image() {
    check_trimmed_image("exfat-trimmed.img", "exFAT");
}
