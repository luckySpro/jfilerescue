//! Small helpers shared across modules.

/// Normalize a volume-relative path: backslashes → `/`, strip drive letters
/// (`D:\x` → `x`), collapse duplicate separators, trim leading/trailing `/`.
pub fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    // Strip Windows drive prefix `X:` or `X:/`.
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        s = s[2..].to_string();
    }
    // Strip `/Volumes/Name` prefix on macOS mount paths.
    if let Some(rest) = s.strip_prefix("/Volumes/") {
        s = match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => String::new(),
        };
    }
    let parts: Vec<&str> = s
        .split('/')
        .filter(|x| !x.is_empty() && *x != ".")
        .collect();
    parts.join("/")
}

/// Lower-case extension without dot; empty when none.
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Replace characters that are illegal in file names on Windows/macOS.
pub fn sanitize_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = out.trim_end_matches(['.', ' ']).to_string();
    out = if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed
    };
    // Reserved device names on Windows.
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out = format!("_{}", out);
    }
    out
}

/// Windows FILETIME (100ns since 1601) → unix seconds.
pub fn filetime_to_unix(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    const EPOCH_DIFF: i64 = 11_644_473_600;
    Some((ft / 10_000_000) as i64 - EPOCH_DIFF)
}

/// Human readable bytes.
pub fn human_bytes(n: u64) -> String {
    const U: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, U[i])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

pub fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub fn le_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}
pub fn be_u32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Decode UTF-16LE, replacing invalid units.
pub fn utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// True when a decoded name looks like garbage (control chars, replacement
/// chars, or no printable content).
pub fn name_looks_damaged(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    let mut bad = 0usize;
    for c in name.chars() {
        if c == '\u{FFFD}' || (c as u32) < 0x20 || (0xD800..=0xDFFF).contains(&(c as u32)) {
            bad += 1;
        }
    }
    bad > 0 || name.chars().all(|c| !c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        assert_eq!(normalize_path("D:\\项目\\2025-合同\\"), "项目/2025-合同");
        assert_eq!(normalize_path("/Volumes/LaCie/Photos/2025"), "Photos/2025");
        assert_eq!(normalize_path("//a///b/"), "a/b");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn ext() {
        assert_eq!(ext_of("a.JPG"), "jpg");
        assert_eq!(ext_of(".hidden"), "");
        assert_eq!(ext_of("noext"), "");
        assert_eq!(ext_of("a.tar.gz"), "gz");
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("a:b?c"), "a_b_c");
        assert_eq!(sanitize_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_name("trail. "), "trail");
    }

    #[test]
    fn filetime() {
        // 2025-01-01T00:00:00Z = (1_735_689_600 + 11_644_473_600) * 10^7
        assert_eq!(
            filetime_to_unix(133_801_632_000_000_000),
            Some(1_735_689_600)
        );
        assert_eq!(filetime_to_unix(0), None);
    }
}
