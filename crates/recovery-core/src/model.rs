//! Shared data model: discovered files, recoverability, sources, filters.

use serde::{Deserialize, Serialize};

/// How a file was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Found via file system metadata (MFT record, directory entry, inode).
    Metadata,
    /// Found via signature carving in unallocated space.
    Carved,
}

/// Coarse recoverability shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsKind {
    Ntfs,
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    Apfs,
    Unknown,
}

impl FsKind {
    pub fn label(&self) -> &'static str {
        match self {
            FsKind::Ntfs => "NTFS",
            FsKind::Fat12 => "FAT12",
            FsKind::Fat16 => "FAT16",
            FsKind::Fat32 => "FAT32",
            FsKind::ExFat => "exFAT",
            FsKind::Apfs => "APFS",
            FsKind::Unknown => "未知",
        }
    }
}

/// A contiguous run of bytes on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub offset: u64,
    pub len: u64,
}

/// One recoverable file as presented to the UI and the recover stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoundFile {
    /// Stable id within a session.
    pub id: u64,
    pub name: String,
    /// Original directory path, `None` for carved files.
    pub dir: Option<String>,
    pub size: u64,
    pub source: Source,
    pub quality: Quality,
    /// 0..=100, fraction of data clusters that have since been reallocated.
    pub overwritten_pct: u8,
    /// Unix seconds; `None` when unknown.
    pub deleted_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub created_at: Option<i64>,
    /// Extension inferred from name or signature (lower case, no dot).
    pub ext: String,
    /// Where the data lives.
    pub extents: Vec<Extent>,
    /// Data is stored inside the metadata record itself (NTFS resident).
    #[serde(default)]
    pub resident: Option<Vec<u8>>,
    /// Name could not be decoded; a synthetic one was assigned.
    #[serde(default)]
    pub name_damaged: bool,
    /// Free-form reason for the quality score.
    #[serde(default)]
    pub note: String,
}

impl FoundFile {
    pub fn full_path(&self) -> String {
        match &self.dir {
            Some(d) if !d.is_empty() => format!("{}/{}", d.trim_end_matches('/'), self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    /// Metadata scan limited to one original directory subtree.
    Directory,
    /// Whole volume / disk.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanDepth {
    /// Metadata only.
    Quick,
    /// Metadata + signature carving of unallocated space.
    Deep,
}

/// Scan request as produced by the UI / CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub mode: ScanMode,
    pub depth: ScanDepth,
    /// Original directory (volume-relative, `/` separated) for `Directory` mode.
    #[serde(default)]
    pub directory: Option<String>,
    /// Extensions (lower-case, no dot). Empty = all.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Include records whose name could not be decoded.
    #[serde(default = "default_true")]
    pub include_damaged_names: bool,
    /// Carve only unallocated clusters (faster) — when false carve everything.
    #[serde(default = "default_true")]
    pub carve_unallocated_only: bool,
    /// Worker threads for carving; 0 = auto.
    #[serde(default)]
    pub threads: usize,
}

fn default_true() -> bool {
    true
}

impl Default for ScanRequest {
    fn default() -> Self {
        Self {
            mode: ScanMode::Full,
            depth: ScanDepth::Quick,
            directory: None,
            extensions: vec![],
            include_damaged_names: true,
            carve_unallocated_only: true,
            threads: 0,
        }
    }
}

/// Filter applied when listing results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResultFilter {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub min_quality: Option<Quality>,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub deleted_after: Option<i64>,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub dir_prefix: Option<String>,
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default = "default_true")]
    pub hide_empty: bool,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: usize,
}

impl ResultFilter {
    pub fn matches(&self, f: &FoundFile) -> bool {
        if self.hide_empty && f.size == 0 {
            return false;
        }
        if let Some(q) = self.min_quality {
            if f.quality < q {
                return false;
            }
        }
        if let Some(s) = self.min_size {
            if f.size < s {
                return false;
            }
        }
        if let Some(t) = self.deleted_after {
            match f.deleted_at {
                Some(d) if d >= t => {}
                _ => return false,
            }
        }
        if let Some(s) = self.source {
            if f.source != s {
                return false;
            }
        }
        if let Some(e) = &self.ext {
            if !f.ext.eq_ignore_ascii_case(e) {
                return false;
            }
        }
        if let Some(p) = &self.dir_prefix {
            let d = f.dir.as_deref().unwrap_or("");
            if !path_has_prefix(d, p) {
                return false;
            }
        }
        if !self.query.is_empty() {
            let q = self.query.to_lowercase();
            let hay = f.full_path().to_lowercase();
            if !hay.contains(&q) {
                return false;
            }
        }
        true
    }
}

/// Case-insensitive, separator-aware path prefix test (`/a/b` matches `/a/b` and `/a/b/c`, not `/a/bc`).
pub fn path_has_prefix(path: &str, prefix: &str) -> bool {
    let p = crate::util::normalize_path(path);
    let pre = crate::util::normalize_path(prefix);
    if pre.is_empty() {
        return true;
    }
    let p_l = p.to_lowercase();
    let pre_l = pre.to_lowercase();
    p_l == pre_l || p_l.starts_with(&(pre_l.clone() + "/"))
}

/// Summary counters for the UI header.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub total: u64,
    pub with_name: u64,
    pub carved: u64,
    pub damaged_name: u64,
    pub bytes: u64,
    pub high: u64,
    pub medium: u64,
    pub low: u64,
}

impl Summary {
    pub fn add(&mut self, f: &FoundFile) {
        self.total += 1;
        self.bytes += f.size;
        match f.source {
            Source::Metadata => self.with_name += 1,
            Source::Carved => self.carved += 1,
        }
        if f.name_damaged {
            self.damaged_name += 1;
        }
        match f.quality {
            Quality::High => self.high += 1,
            Quality::Medium => self.medium += 1,
            Quality::Low => self.low += 1,
        }
    }
}
