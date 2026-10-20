//! Scan session persistence: results are saved as JSON (gzip-free, simple)
//! under the app data directory so the user can close the app and come back.
//! Writes are atomic (temp + rename).

use crate::model::{FoundFile, ResultFilter, ScanRequest, Summary};
use crate::scan::ScanOutcome;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    Completed,
    Aborted,
    Failed,
}

/// Which device this session belongs to, so it can be re-opened for recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRef {
    /// Platform path (e.g. `\\.\PhysicalDrive1`, `/dev/rdisk4`) or an image file path.
    pub path: String,
    /// Stable identity for sanity checks on reopen (serial / UUID / size).
    pub identity: String,
    pub size: u64,
    pub sector_size: u32,
    pub partition: Option<u32>,
    pub display_name: String,
    #[serde(default)]
    pub is_image: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub state: SessionState,
    pub device: DeviceRef,
    pub request: ScanRequest,
    pub summary: Summary,
    pub fs: String,
    pub volume_label: Option<String>,
    pub volume_offset: u64,
    pub volume_len: u64,
    pub recovered: u64,
    pub read_errors: u64,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub meta: SessionMeta,
    pub files: Vec<FoundFile>,
}

pub struct SessionStore {
    root: PathBuf,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn new_id() -> String {
        let t = now();
        let r = std::process::id() as u64 ^ (t as u64).rotate_left(17);
        format!("{}-{:04x}", t, r & 0xFFFF)
    }

    pub fn create(&self, device: DeviceRef, request: ScanRequest) -> Result<SessionMeta> {
        let meta = SessionMeta {
            id: Self::new_id(),
            created_at: now(),
            finished_at: None,
            state: SessionState::Running,
            device,
            request,
            summary: Summary::default(),
            fs: "unknown".into(),
            volume_label: None,
            volume_offset: 0,
            volume_len: 0,
            recovered: 0,
            read_errors: 0,
            warning: None,
            error: None,
        };
        self.write_meta(&meta)?;
        Ok(meta)
    }

    pub fn write_meta(&self, meta: &SessionMeta) -> Result<()> {
        let data = serde_json::to_vec_pretty(meta).map_err(|e| Error::Session(e.to_string()))?;
        atomic_write(&self.dir(&meta.id).join("meta.json"), &data)
    }

    pub fn complete(
        &self,
        meta: &mut SessionMeta,
        outcome: &ScanOutcome,
        state: SessionState,
    ) -> Result<()> {
        meta.state = state;
        meta.finished_at = Some(now());
        meta.summary = outcome.summary.clone();
        meta.fs = outcome.fs.label().to_string();
        meta.volume_label = outcome.volume_label.clone();
        meta.volume_offset = outcome.volume_offset;
        meta.volume_len = outcome.volume_len;
        meta.read_errors = outcome.read_errors;
        meta.warning = outcome.warning.clone();
        let files =
            serde_json::to_vec(&outcome.files).map_err(|e| Error::Session(e.to_string()))?;
        atomic_write(&self.dir(&meta.id).join("files.json"), &files)?;
        self.write_meta(meta)
    }

    pub fn fail(&self, meta: &mut SessionMeta, err: &str) -> Result<()> {
        meta.state = SessionState::Failed;
        meta.finished_at = Some(now());
        meta.error = Some(err.to_string());
        self.write_meta(meta)
    }

    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        let mut out = vec![];
        if !self.root.exists() {
            return Ok(out);
        }
        for e in std::fs::read_dir(&self.root)? {
            let e = e?;
            let p = e.path().join("meta.json");
            if let Ok(txt) = std::fs::read(&p) {
                if let Ok(m) = serde_json::from_slice::<SessionMeta>(&txt) {
                    out.push(m);
                }
            }
        }
        out.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(out)
    }

    pub fn load_meta(&self, id: &str) -> Result<SessionMeta> {
        let txt = std::fs::read(self.dir(id).join("meta.json"))
            .map_err(|_| Error::Session(format!("会话 {} 不存在", id)))?;
        serde_json::from_slice(&txt).map_err(|e| Error::Session(e.to_string()))
    }

    pub fn load_files(&self, id: &str) -> Result<Vec<FoundFile>> {
        let p = self.dir(id).join("files.json");
        if !p.exists() {
            return Ok(vec![]);
        }
        let txt = std::fs::read(p)?;
        serde_json::from_slice(&txt).map_err(|e| Error::Session(e.to_string()))
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        Ok(Session {
            meta: self.load_meta(id)?,
            files: self.load_files(id)?,
        })
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let d = self.dir(id);
        if d.exists() {
            std::fs::remove_dir_all(d)?;
        }
        Ok(())
    }

    pub fn total_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            std::fs::read_dir(p)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| {
                            e.metadata()
                                .map(|m| if m.is_dir() { walk(&e.path()) } else { m.len() })
                                .unwrap_or(0)
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.root)
    }
}

/// Directory tree node for the results sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub name: String,
    pub path: String,
    pub count: u64,
    pub children: Vec<TreeNode>,
}

pub fn build_tree(files: &[FoundFile]) -> TreeNode {
    #[derive(Default)]
    struct N {
        count: u64,
        children: BTreeMap<String, N>,
    }
    let mut root = N::default();
    for f in files {
        let Some(dir) = &f.dir else { continue };
        let mut cur = &mut root;
        cur.count += 1;
        for seg in dir.split('/').filter(|s| !s.is_empty()) {
            cur = cur.children.entry(seg.to_string()).or_default();
            cur.count += 1;
        }
    }
    fn conv(name: &str, path: &str, n: &N) -> TreeNode {
        TreeNode {
            name: name.into(),
            path: path.into(),
            count: n.count,
            children: n
                .children
                .iter()
                .map(|(k, v)| {
                    conv(
                        k,
                        &if path.is_empty() {
                            k.clone()
                        } else {
                            format!("{}/{}", path, k)
                        },
                        v,
                    )
                })
                .collect(),
        }
    }
    conv("", "", &root)
}

/// Per-extension counts for carved files (the "by type" view).
pub fn type_counts(files: &[FoundFile]) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for f in files {
        *m.entry(if f.ext.is_empty() {
            "(无扩展名)".into()
        } else {
            f.ext.clone()
        })
        .or_insert(0) += 1;
    }
    m
}

/// Apply a filter with paging. Returns (page, total matching).
pub fn query<'a>(files: &'a [FoundFile], filter: &ResultFilter) -> (Vec<&'a FoundFile>, usize) {
    let all: Vec<&FoundFile> = files.iter().filter(|f| filter.matches(f)).collect();
    let total = all.len();
    let limit = if filter.limit == 0 { 500 } else { filter.limit };
    let page = all.into_iter().skip(filter.offset).take(limit).collect();
    (page, total)
}

/// CSV export of the result list.
pub fn export_csv(files: &[FoundFile]) -> String {
    let mut s = String::from(
        "\u{FEFF}id,name,path,size,source,quality,overwritten_pct,deleted_at,modified_at,note\n",
    );
    for f in files {
        let esc = |v: &str| format!("\"{}\"", v.replace('"', "\"\""));
        s.push_str(&format!(
            "{},{},{},{},{:?},{:?},{},{},{},{}\n",
            f.id,
            esc(&f.name),
            esc(f.dir.as_deref().unwrap_or("")),
            f.size,
            f.source,
            f.quality,
            f.overwritten_pct,
            f.deleted_at.map(|t| t.to_string()).unwrap_or_default(),
            f.modified_at.map(|t| t.to_string()).unwrap_or_default(),
            esc(&f.note)
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Quality, Source};

    fn f(id: u64, dir: &str, name: &str) -> FoundFile {
        FoundFile {
            id,
            name: name.into(),
            dir: Some(dir.into()),
            size: 10,
            source: Source::Metadata,
            quality: Quality::High,
            overwritten_pct: 0,
            deleted_at: None,
            modified_at: None,
            created_at: None,
            ext: crate::util::ext_of(name),
            extents: vec![],
            resident: None,
            name_damaged: false,
            note: String::new(),
        }
    }

    #[test]
    fn tree_counts() {
        let files = vec![f(1, "a", "x.txt"), f(2, "a/b", "y.txt"), f(3, "c", "z.txt")];
        let t = build_tree(&files);
        assert_eq!(t.count, 3);
        let a = t.children.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.count, 2);
        assert_eq!(a.children[0].path, "a/b");
    }

    #[test]
    fn store_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let dev = DeviceRef {
            path: "img".into(),
            identity: "x".into(),
            size: 1,
            sector_size: 512,
            partition: None,
            display_name: "d".into(),
            is_image: true,
        };
        let mut meta = store.create(dev, ScanRequest::default()).unwrap();
        let outcome = ScanOutcome {
            fs: crate::model::FsKind::Ntfs,
            volume_label: None,
            volume_offset: 0,
            volume_len: 1,
            files: vec![f(1, "a", "x.txt")],
            summary: Summary::default(),
            read_errors: 0,
            carve_only: false,
            warning: None,
        };
        store
            .complete(&mut meta, &outcome, SessionState::Completed)
            .unwrap();
        let s = store.load(&meta.id).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(store.list().unwrap().len(), 1);
        store.delete(&meta.id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
