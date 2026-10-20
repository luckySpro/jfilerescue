//! Length resolution for carved files: footer search and structural parsers.

use super::signatures::{HeaderPattern, LengthRule, Signature};
use crate::device::BlockDevice;
use crate::util::{be_u32, le_u16, le_u32, le_u64};
use memchr::memmem;

pub struct Resolved {
    pub len: Option<u64>,
    /// True when the end was located by structure/footer rather than max size.
    pub verified: bool,
}

/// Read up to `max` bytes starting at `abs`, preferring the in-memory probe.
fn window<'a, D: BlockDevice + ?Sized>(
    dev: &D,
    abs: u64,
    probe: &'a [u8],
    want: usize,
) -> std::borrow::Cow<'a, [u8]> {
    if probe.len() >= want {
        return std::borrow::Cow::Borrowed(&probe[..want]);
    }
    let want = want.min((dev.size() - abs) as usize);
    match dev.read_vec(abs, want) {
        Ok(v) => std::borrow::Cow::Owned(v),
        Err(_) => std::borrow::Cow::Borrowed(probe),
    }
}

pub fn resolve_len<D: BlockDevice + ?Sized>(
    dev: &D,
    sig: &Signature,
    abs: u64,
    probe: &[u8],
    _avail: usize,
) -> Resolved {
    let max = sig.max_size.min(dev.size() - abs);
    let r = match &sig.length {
        LengthRule::Fixed { size } => Resolved {
            len: Some(*size),
            verified: true,
        },
        LengthRule::MaxSize => Resolved {
            len: Some(max),
            verified: false,
        },
        LengthRule::Footer { hex } => footer(dev, hex, abs, probe, max),
        LengthRule::Parser { name } => match name.as_str() {
            "jpeg" => jpeg(dev, abs, probe, max),
            "png" => png(dev, abs, probe, max),
            "bmp" => simple_le32(probe, 2, max),
            "riff" => simple_le32(probe, 4, max).map_len(|l| l + 8),
            "bmff" => bmff(dev, abs, probe, max),
            "zip" => zip(dev, abs, probe, max),
            "pdf" => pdf(dev, abs, probe, max),
            "ole" => ole(dev, abs, probe, max),
            "sevenzip" => sevenzip(probe, max),
            "tar" => tar(dev, abs, probe, max),
            "asf" => asf(probe, max),
            "pe" => pe(dev, abs, probe, max),
            "sqlite" => sqlite(probe, max),
            "text" => text(dev, abs, probe, max),
            _ => Resolved {
                len: Some(max),
                verified: false,
            },
        },
    };
    // Clamp.
    match r.len {
        Some(l) if l > max => Resolved {
            len: Some(max),
            verified: false,
        },
        _ => r,
    }
}

trait MapLen {
    fn map_len(self, f: impl Fn(u64) -> u64) -> Self;
}
impl MapLen for Resolved {
    fn map_len(self, f: impl Fn(u64) -> u64) -> Self {
        Resolved {
            len: self.len.map(f),
            verified: self.verified,
        }
    }
}

fn simple_le32(probe: &[u8], off: usize, max: u64) -> Resolved {
    if probe.len() < off + 4 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let l = le_u32(probe, off) as u64;
    if l == 0 || l > max {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    Resolved {
        len: Some(l),
        verified: true,
    }
}

fn footer<D: BlockDevice + ?Sized>(
    dev: &D,
    hex: &str,
    abs: u64,
    probe: &[u8],
    max: u64,
) -> Resolved {
    let (pat, _) = HeaderPattern {
        hex: hex.into(),
        offset: 0,
    }
    .literal_prefix();
    // Search progressively larger windows.
    let mut want = (1usize << 20).min(max as usize);
    loop {
        let w = window(dev, abs, probe, want);
        if let Some(p) = memmem::find(&w[1..], &pat) {
            return Resolved {
                len: Some((p + 1 + pat.len()) as u64),
                verified: true,
            };
        }
        if want as u64 >= max || w.len() < want {
            return Resolved {
                len: Some(max),
                verified: false,
            };
        }
        want = (want * 4).min(max as usize);
    }
}

fn jpeg<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Walk segments to SOS, then search for EOI not followed by garbage.
    let w = window(dev, abs, probe, (4 << 20).min(max as usize));
    let mut p = 2usize;
    let mut sos = None;
    while p + 4 <= w.len() {
        if w[p] != 0xFF {
            break;
        }
        let m = w[p + 1];
        if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            p += 2;
            continue;
        }
        let l = u16::from_be_bytes([w[p + 2], w[p + 3]]) as usize;
        if l < 2 {
            break;
        }
        if m == 0xDA {
            sos = Some(p + 2 + l);
            break;
        }
        p += 2 + l;
    }
    // Without a SOS marker (truncated/odd header) still try to find an EOI,
    // but report the result as unverified.
    let (start, structured) = match sos {
        Some(s) => (s, true),
        None => (2usize, false),
    };
    // Scan for FF D9 in entropy data.
    let mut want = w.len();
    loop {
        let w = window(dev, abs, probe, want);
        if let Some(i) = memmem::find(&w[start.min(w.len())..], &[0xFF, 0xD9]) {
            return Resolved {
                len: Some((start + i + 2) as u64),
                verified: structured,
            };
        }
        if want as u64 >= max {
            return Resolved {
                len: Some(max),
                verified: false,
            };
        }
        want = (want * 4).min(max as usize);
    }
}

fn png<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    let mut want = (1usize << 20).min(max as usize);
    loop {
        let w = window(dev, abs, probe, want);
        let mut p = 8usize;
        let mut need_more = false;
        while p + 12 <= w.len() {
            let l = be_u32(&w, p) as usize;
            let t = &w[p + 4..p + 8];
            if t == b"IEND" {
                return Resolved {
                    len: Some((p + 12) as u64),
                    verified: true,
                };
            }
            if !t.iter().all(|c| c.is_ascii_alphabetic()) {
                return Resolved {
                    len: Some(max),
                    verified: false,
                };
            }
            p += 12 + l;
            if p > w.len() {
                need_more = true;
                break;
            }
        }
        if !need_more && p + 12 > w.len() {
            need_more = true;
        }
        if !need_more || want as u64 >= max {
            return Resolved {
                len: Some(max),
                verified: false,
            };
        }
        want = (want * 4).min(max as usize);
    }
}

fn bmff<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Follow top-level boxes by size until data runs out or a non-ASCII type appears.
    let mut pos: u64 = 0;
    let mut verified = false;
    let mut seen_mdat = false;
    let mut seen_moov = false;
    for _ in 0..64 {
        let hdr = window(dev, abs + pos, if pos == 0 { probe } else { &[] }, 16);
        if hdr.len() < 8 {
            break;
        }
        let mut size = be_u32(&hdr, 0) as u64;
        let t = &hdr[4..8];
        if !t
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b' ' || *c == 0xA9)
        {
            break;
        }
        if size == 1 {
            if hdr.len() < 16 {
                break;
            }
            size = u64::from_be_bytes([
                hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15],
            ]);
        } else if size == 0 {
            // Box extends to end of file: unknown.
            return Resolved {
                len: Some(max),
                verified: false,
            };
        }
        if size < 8 || pos + size > max {
            break;
        }
        if t == b"mdat" {
            seen_mdat = true;
        }
        if t == b"moov" {
            seen_moov = true;
        }
        pos += size;
        verified = seen_mdat && seen_moov;
        if pos >= max {
            break;
        }
    }
    if pos == 0 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    Resolved {
        len: Some(pos),
        verified,
    }
}

fn zip<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Find End Of Central Directory record (PK\x05\x06) then validate it
    // points inside the file; total = eocd_pos + 22 + comment_len.
    let mut want = (1usize << 20).min(max as usize);
    loop {
        let w = window(dev, abs, probe, want);
        let mut search_from = 0usize;
        while let Some(i) = memmem::find(&w[search_from..], b"PK\x05\x06") {
            let p = search_from + i;
            if p + 22 <= w.len() {
                let cd_size = le_u32(&w, p + 12) as u64;
                let cd_off = le_u32(&w, p + 16) as u64;
                let comment = le_u16(&w, p + 20) as u64;
                let plausible = cd_off + cd_size == p as u64 || cd_off < p as u64;
                if plausible {
                    let total = p as u64 + 22 + comment;
                    return Resolved {
                        len: Some(total.min(max)),
                        verified: cd_off + cd_size == p as u64,
                    };
                }
            }
            search_from = p + 4;
        }
        if want as u64 >= max || w.len() < want {
            return Resolved {
                len: Some(max),
                verified: false,
            };
        }
        want = (want * 4).min(max as usize);
    }
}

fn pdf<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Last %%EOF within a growing window; PDFs can have multiple (incremental updates).
    let mut want = (1usize << 20).min(max as usize);
    let mut best: Option<usize> = None;
    loop {
        let w = window(dev, abs, probe, want);
        let mut from = best.unwrap_or(0);
        let mut last = best;
        while let Some(i) = memmem::find(&w[from..], b"%%EOF") {
            last = Some(from + i + 5);
            from = from + i + 5;
        }
        best = last;
        if let Some(b) = best {
            // Stop when nothing but whitespace/zeros follows the last EOF in a 64 KiB lookahead
            let tail = &w[b..(b + 65536).min(w.len())];
            let more =
                memmem::find(tail, b"%%EOF").is_some() || memmem::find(tail, b"obj").is_some();
            if !more {
                let mut end = b;
                while end < w.len() && matches!(w[end], b'\r' | b'\n' | b' ') {
                    end += 1;
                }
                return Resolved {
                    len: Some(end as u64),
                    verified: true,
                };
            }
        }
        if want as u64 >= max || w.len() < want {
            return match best {
                Some(b) => Resolved {
                    len: Some(b as u64),
                    verified: true,
                },
                None => Resolved {
                    len: Some(max),
                    verified: false,
                },
            };
        }
        want = (want * 4).min(max as usize);
    }
}

fn ole<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Sector size from header; walk FAT via DIFAT to count sectors.
    let w = window(dev, abs, probe, 512);
    if w.len() < 512 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let sector_shift = le_u16(&w, 30);
    if !(9..=12).contains(&sector_shift) {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let ss = 1u64 << sector_shift;
    let fat_sectors = le_u32(&w, 44) as u64;
    if fat_sectors == 0 || fat_sectors > 1 << 20 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    // Each FAT sector covers ss/4 sectors; upper bound on file size.
    let entries = fat_sectors * (ss / 4);
    // Read the DIFAT (first 109 entries) and count non-free entries in each FAT sector.
    let mut used = 0u64;
    for i in 0..109usize.min(fat_sectors as usize) {
        let fs = le_u32(&w, 76 + i * 4) as u64;
        if fs >= 0xFFFF_FFFE {
            break;
        }
        let Ok(fat) = dev.read_vec(abs + (fs + 1) * ss, ss as usize) else {
            break;
        };
        for e in fat.as_chunks::<4>().0 {
            if le_u32(e, 0) != 0xFFFF_FFFF {
                used += 1;
            }
        }
    }
    let total = ((used.max(1)) + 1) * ss;
    if used > 0 && total <= max {
        Resolved {
            len: Some(total),
            verified: true,
        }
    } else {
        Resolved {
            len: Some(((entries + 1) * ss).min(max)),
            verified: false,
        }
    }
}

fn sevenzip(probe: &[u8], max: u64) -> Resolved {
    if probe.len() < 32 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let next_off = le_u64(probe, 12);
    let next_size = le_u64(probe, 20);
    let total = 32 + next_off + next_size;
    if total > 32 && total <= max {
        Resolved {
            len: Some(total),
            verified: true,
        }
    } else {
        Resolved {
            len: Some(max),
            verified: false,
        }
    }
}

fn tar<D: BlockDevice + ?Sized>(dev: &D, abs: u64, _probe: &[u8], max: u64) -> Resolved {
    let mut pos = 0u64;
    while let Ok(h) = dev.read_vec(abs + pos, 512) {
        if h.iter().all(|b| *b == 0) {
            return Resolved {
                len: Some((pos + 1024).min(max)),
                verified: true,
            };
        }
        if &h[257..262] != b"ustar" {
            break;
        }
        let size_str = String::from_utf8_lossy(&h[124..136]);
        let size = u64::from_str_radix(size_str.trim_matches(|c: char| c == '\0' || c == ' '), 8)
            .unwrap_or(0);
        pos += 512 + size.div_ceil(512) * 512;
        if pos >= max {
            break;
        }
    }
    Resolved {
        len: Some(pos.max(512).min(max)),
        verified: false,
    }
}

fn asf(probe: &[u8], max: u64) -> Resolved {
    if probe.len() < 24 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    // Header object size then the data object; file size lives in File Properties, approximate by walking.
    let hdr = le_u64(probe, 16);
    if hdr < 30 || hdr > max {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    Resolved {
        len: Some(max),
        verified: false,
    }
}

fn pe<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    let w = window(dev, abs, probe, 4096);
    if w.len() < 64 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let pe_off = le_u32(&w, 60) as usize;
    let Ok(h) = dev.read_vec(abs + pe_off as u64, 1024) else {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    };
    if &h[0..4] != b"PE\0\0" {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let nsec = le_u16(&h, 6) as usize;
    let opt = le_u16(&h, 20) as usize;
    let st = 24 + opt;
    let mut end = 0u64;
    for i in 0..nsec.min(96) {
        let s = st + i * 40;
        if s + 40 > h.len() {
            break;
        }
        let raw_size = le_u32(&h, s + 16) as u64;
        let raw_ptr = le_u32(&h, s + 20) as u64;
        end = end.max(raw_ptr + raw_size);
    }
    if end > 0 && end <= max {
        Resolved {
            len: Some(end),
            verified: true,
        }
    } else {
        Resolved {
            len: Some(max),
            verified: false,
        }
    }
}

fn sqlite(probe: &[u8], max: u64) -> Resolved {
    if probe.len() < 32 {
        return Resolved {
            len: Some(max),
            verified: false,
        };
    }
    let ps = u16::from_be_bytes([probe[16], probe[17]]) as u64;
    let page = if ps == 1 { 65536 } else { ps };
    let pages = be_u32(probe, 28) as u64;
    if page >= 512 && pages > 0 {
        let t = page * pages;
        if t <= max {
            return Resolved {
                len: Some(t),
                verified: true,
            };
        }
    }
    Resolved {
        len: Some(max),
        verified: false,
    }
}

fn text<D: BlockDevice + ?Sized>(dev: &D, abs: u64, probe: &[u8], max: u64) -> Resolved {
    // Extend while bytes look like UTF-8 text; stop at first NUL run.
    let w = window(dev, abs, probe, (1usize << 20).min(max as usize));
    let mut end = 0;
    for (i, b) in w.iter().enumerate() {
        if *b == 0 {
            break;
        }
        end = i + 1;
    }
    // Trim to last newline for cleanliness.
    let cut = w[..end]
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(end);
    Resolved {
        len: Some(cut.max(1) as u64),
        verified: end < w.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;

    #[test]
    fn png_len() {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&[0u8; 13 + 4]);
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(b"IEND");
        v.extend_from_slice(&[0u8; 4]);
        let total = v.len() as u64;
        v.extend_from_slice(&[0xAA; 100]);
        let dev = MemDevice::new(v.clone());
        let r = png(&dev, 0, &v, 10_000);
        assert_eq!(r.len, Some(total));
        assert!(r.verified);
    }

    #[test]
    fn zip_len() {
        let mut v = b"PK\x03\x04".to_vec();
        v.extend_from_slice(&[0u8; 26]);
        let cd_off = v.len() as u32;
        v.extend_from_slice(b"PK\x01\x02");
        v.extend_from_slice(&[0u8; 42]);
        let cd_size = (v.len() as u32) - cd_off;
        v.extend_from_slice(b"PK\x05\x06");
        v.extend_from_slice(&[0u8; 8]);
        v.extend_from_slice(&cd_size.to_le_bytes());
        v.extend_from_slice(&cd_off.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        let total = v.len() as u64;
        v.extend_from_slice(&[0x55; 64]);
        let dev = MemDevice::new(v.clone());
        let r = zip(&dev, 0, &v, 100_000);
        assert_eq!(r.len, Some(total));
        assert!(r.verified);
    }

    #[test]
    fn jpeg_len() {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        v.extend_from_slice(&[0x11; 50]);
        v.extend_from_slice(&[0xFF, 0xD9]);
        let total = v.len() as u64;
        v.extend_from_slice(&[0u8; 32]);
        let dev = MemDevice::new(v.clone());
        let r = jpeg(&dev, 0, &v, 10_000);
        assert_eq!(r.len, Some(total));
    }
}
