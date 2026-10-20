//! Signature carving: scan (unallocated) ranges for file headers and resolve
//! their length via footer / format-specific parsing.

pub mod resolve;
pub mod signatures;

use crate::device::BlockDevice;
use crate::fs::AllocBitmap;
use crate::model::{Extent, FoundFile, Quality, Source};
use crate::progress::{Phase, SharedProgress};
use crate::Result;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use signatures::{Signature, SignatureLibrary};
use std::sync::{Arc, Mutex};

/// Bytes read per worker chunk.
const CHUNK: u64 = 8 << 20;
/// Overlap so headers straddling a chunk boundary are not missed.
const OVERLAP: u64 = 64 << 10;

pub struct Carver {
    sigs: Vec<Signature>,
    ac: AhoCorasick,
    /// pattern index -> (signature index)
    pat_map: Vec<usize>,
    pat_offsets: Vec<usize>,
    align: u64,
}

impl Carver {
    pub fn new(lib: &SignatureLibrary, only_exts: &[String], align: u64) -> Result<Self> {
        let sigs: Vec<Signature> = lib
            .enabled()
            .into_iter()
            .filter(|s| {
                only_exts.is_empty()
                    || s.extensions
                        .iter()
                        .any(|e| only_exts.iter().any(|o| o.eq_ignore_ascii_case(e)))
            })
            .cloned()
            .collect();
        let mut patterns: Vec<Vec<u8>> = vec![];
        let mut pat_map = vec![];
        let mut pat_offsets = vec![];
        for (i, s) in sigs.iter().enumerate() {
            for h in &s.headers {
                // Wildcard prefix handled by trimming leading `??` bytes and remembering offset.
                let (lit, lead) = h.literal_prefix();
                if lit.is_empty() {
                    continue;
                }
                patterns.push(lit);
                pat_map.push(i);
                pat_offsets.push(lead + h.offset);
            }
        }
        let ac = AhoCorasickBuilder::new()
            .match_kind(MatchKind::Standard)
            .build(&patterns)
            .map_err(|e| crate::Error::InvalidInput(format!("签名库编译失败: {}", e)))?;
        Ok(Self {
            sigs,
            ac,
            pat_map,
            pat_offsets,
            align: align.max(1),
        })
    }

    pub fn signature_count(&self) -> usize {
        self.sigs.len()
    }

    /// Carve the given ranges. `bitmap` (if any) is used to skip allocated
    /// clusters and to score overlap. Returns files with ids starting at `first_id`.
    pub fn carve<D: BlockDevice + 'static + ?Sized>(
        &self,
        dev: Arc<D>,
        ranges: &[Extent],
        bitmap: Option<&AllocBitmap>,
        threads: usize,
        first_id: u64,
        progress: &SharedProgress,
    ) -> Result<Vec<FoundFile>> {
        let total: u64 = ranges.iter().map(|r| r.len).sum();
        progress.set_phase(Phase::Carve, total);
        progress.set_message(format!("签名雕刻（{} 种签名）", self.sigs.len()));

        // Split ranges into work items.
        let mut items: Vec<Extent> = vec![];
        for r in ranges {
            let mut off = r.offset;
            let end = r.offset + r.len;
            while off < end {
                let len = CHUNK.min(end - off);
                items.push(Extent { offset: off, len });
                off += len;
            }
        }
        let items = Arc::new(Mutex::new(items));
        let found: Arc<Mutex<Vec<FoundFile>>> = Arc::new(Mutex::new(vec![]));
        let nthreads = if threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(16)
        } else {
            threads
        };
        let dev_size = dev.size();

        std::thread::scope(|s| {
            for _ in 0..nthreads {
                let items = Arc::clone(&items);
                let found = Arc::clone(&found);
                let dev = Arc::clone(&dev);
                let progress = Arc::clone(progress);
                s.spawn(move || {
                    let mut buf = vec![0u8; (CHUNK + OVERLAP) as usize];
                    loop {
                        if progress.checkpoint().is_err() {
                            return;
                        }
                        let item = { items.lock().ok().and_then(|mut g| g.pop()) };
                        let Some(item) = item else { return };
                        let read_len = (item.len + OVERLAP).min(dev_size - item.offset) as usize;
                        let got = dev.read_lossy(item.offset, &mut buf[..read_len]);
                        if got < read_len {
                            progress.add_read_error();
                        }
                        progress.add_bytes(read_len as u64);
                        let mut local = vec![];
                        self.scan_chunk(
                            &*dev,
                            &buf[..read_len],
                            item.offset,
                            item.len,
                            bitmap,
                            &mut local,
                        );
                        if !local.is_empty() {
                            progress.add_found(local.len() as u64);
                            if let Ok(mut g) = found.lock() {
                                g.extend(local);
                            }
                        }
                        progress.add_done(item.len);
                    }
                });
            }
        });
        if progress.is_cancelled() {
            return Err(crate::Error::Cancelled);
        }

        let mut files = Arc::try_unwrap(found)
            .map(|m| m.into_inner().unwrap_or_default())
            .unwrap_or_default();
        files.sort_by_key(|f| f.extents.first().map(|e| e.offset).unwrap_or(0));
        // Drop a hit entirely contained in the previous hit of the same type (e.g. thumbnail JPEG in JPEG).
        let mut out: Vec<FoundFile> = Vec::with_capacity(files.len());
        for f in files {
            if let Some(prev) = out.last() {
                let pe = prev.extents[0];
                let fe = f.extents[0];
                if prev.ext == f.ext
                    && fe.offset >= pe.offset
                    && fe.offset + fe.len <= pe.offset + pe.len
                {
                    continue;
                }
            }
            out.push(f);
        }
        for (i, f) in out.iter_mut().enumerate() {
            f.id = first_id + i as u64;
            f.name = format!("carved_{:06}.{}", first_id + i as u64, f.ext);
        }
        Ok(out)
    }

    fn scan_chunk<D: BlockDevice + ?Sized>(
        &self,
        dev: &D,
        buf: &[u8],
        base: u64,
        main_len: u64,
        bitmap: Option<&AllocBitmap>,
        out: &mut Vec<FoundFile>,
    ) {
        let mut last_end: u64 = 0;
        for m in self.ac.find_overlapping_iter(buf) {
            let pat = m.pattern().as_usize();
            let sig = &self.sigs[self.pat_map[pat]];
            let lead = self.pat_offsets[pat];
            if m.start() < lead {
                continue;
            }
            let start_in_buf = m.start() - lead;
            let abs = base + start_in_buf as u64;
            if (start_in_buf as u64) >= main_len {
                break; // in overlap region; next chunk owns it
            }
            if abs % self.align != 0 && sig.aligned {
                continue;
            }
            if abs < last_end {
                continue; // inside previous carved file
            }
            // Full header match including wildcards.
            let hdr = &sig
                .headers
                .iter()
                .find(|h| h.literal_prefix().0 == buf[m.start()..m.end()])
                .cloned();
            let Some(h) = hdr else { continue };
            if !h.matches(&buf[start_in_buf..]) {
                continue;
            }
            let avail = buf.len() - start_in_buf;
            let probe = &buf[start_in_buf..];
            let resolved = resolve::resolve_len(dev, sig, abs, probe, avail);
            let Some(len) = resolved.len else { continue };
            if len < sig.min_size || len > sig.max_size {
                continue;
            }
            let ext = sig.extensions[0].clone();
            let extents = vec![Extent { offset: abs, len }];
            let ov = bitmap.map(|b| b.overwritten_pct(&extents)).unwrap_or(0);
            let (quality, note) = match (resolved.verified, ov) {
                (true, 0) => (Quality::High, "结构校验通过".into()),
                (true, _) => (
                    Quality::Medium,
                    format!("结构校验通过，{}% 数据簇已分配", ov),
                ),
                (false, 0) => (Quality::Medium, "按最大大小截取，未验证结尾".into()),
                (false, _) => (Quality::Low, format!("未验证结尾，{}% 数据簇已分配", ov)),
            };
            out.push(FoundFile {
                id: 0,
                name: String::new(),
                dir: None,
                size: len,
                source: Source::Carved,
                quality,
                overwritten_pct: ov,
                deleted_at: None,
                modified_at: None,
                created_at: None,
                ext,
                extents,
                resident: None,
                name_damaged: false,
                note,
            });
            last_end = abs + len;
        }
    }
}
