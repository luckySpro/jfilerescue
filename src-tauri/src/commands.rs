use crate::state::{ActiveRecover, ActiveScan, AppState};
use recovery_core::carve::signatures::Signature;
use recovery_core::device::BlockDevice;
use recovery_core::model::{FoundFile, ResultFilter, ScanRequest};
use recovery_core::progress::{Phase, ProgressSnapshot};
use recovery_core::recover::{RecoverOptions, RecoverReport};
use recovery_core::session::{DeviceRef, SessionMeta, SessionState, TreeNode};
use recovery_platform::{DiskEntry, PrivilegeState, WriterProcess};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::State;

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------- platform ----------

#[tauri::command]
pub fn privilege_state(st: State<AppState>) -> PrivilegeState {
    st.platform.privilege_state()
}

#[tauri::command]
pub fn list_disks(st: State<AppState>) -> R<Vec<DiskEntry>> {
    st.platform.list_disks().map_err(err)
}

#[tauri::command]
pub fn busy_writers(st: State<AppState>, volume: String) -> Vec<WriterProcess> {
    st.platform.busy_writers(&volume)
}

#[tauri::command]
pub fn relaunch_elevated(st: State<AppState>, app: tauri::AppHandle) -> R<()> {
    st.platform.relaunch_elevated().map_err(err)?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn open_privacy_settings(st: State<AppState>) -> R<()> {
    st.platform.open_privacy_settings().map_err(err)
}

// ---------- scan ----------

#[derive(Deserialize)]
pub struct StartScanArgs {
    pub source: String,
    pub partition: Option<u32>,
    pub request: ScanRequest,
    pub display_name: Option<String>,
}

#[tauri::command]
pub fn start_scan(st: State<AppState>, args: StartScanArgs) -> R<String> {
    if st
        .scan
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.done.lock().unwrap().is_none())
        .unwrap_or(false)
    {
        return Err("已有扫描正在进行".into());
    }
    if st
        .recover
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.result.lock().unwrap().is_none())
        .unwrap_or(false)
    {
        return Err("恢复正在进行，请等待完成".into());
    }
    if args.request.mode == recovery_core::model::ScanMode::Directory
        && args
            .request
            .directory
            .as_deref()
            .map(|d| d.trim().is_empty())
            .unwrap_or(true)
    {
        return Err("指定目录找回需要填写原目录路径".into());
    }
    let dev = st.device(&args.source)?;
    let dev_ref = DeviceRef {
        path: args.source.clone(),
        identity: st
            .platform
            .disk_identity_of_device(&args.source)
            .unwrap_or_else(|| format!("{}:{}", args.source, dev.size())),
        size: dev.size(),
        sector_size: dev.sector_size(),
        partition: args.partition,
        display_name: args.display_name.unwrap_or_else(|| dev.describe()),
        is_image: Path::new(&args.source).is_file(),
    };
    let mut meta = st
        .store
        .create(dev_ref, args.request.clone())
        .map_err(err)?;
    let progress = AppState::new_progress();
    let done = Arc::new(Mutex::new(None));
    let session_id = meta.id.clone();
    st.locked_sources.lock().unwrap().push(args.source.clone());
    *st.scan.lock().unwrap() = Some(ActiveScan {
        session_id: session_id.clone(),
        progress: Arc::clone(&progress),
        source_path: args.source.clone(),
        done: Arc::clone(&done),
    });

    let sigs = st.sigs.lock().unwrap().clone();
    let store_root = st.store.root().to_path_buf();
    let req = args.request;
    let partition = args.partition;
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            let store = recovery_core::session::SessionStore::new(store_root);
            let res = recovery_core::scan::run_scan(dev, partition, &req, &sigs, &progress);
            let out = match res {
                Ok(outcome) => {
                    let state = if progress.is_cancelled() {
                        SessionState::Aborted
                    } else {
                        SessionState::Completed
                    };
                    store
                        .complete(&mut meta, &outcome, state)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                Err(recovery_core::Error::Cancelled) => {
                    progress.set_phase(Phase::Cancelled, 1);
                    // Keep whatever was collected? The scan returns nothing on cancel; mark aborted.
                    meta.state = SessionState::Aborted;
                    meta.error = Some("用户取消".into());
                    store
                        .write_meta(&meta)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                Err(e) => {
                    progress.set_phase(Phase::Failed, 1);
                    let _ = store.fail(&mut meta, &e.to_string());
                    Err(e.to_string())
                }
            };
            *done.lock().unwrap() = Some(out);
        })
        .map_err(err)?;
    Ok(session_id)
}

#[derive(Serialize)]
pub struct ScanStatus {
    pub session_id: Option<String>,
    pub running: bool,
    pub progress: ProgressSnapshot,
    pub error: Option<String>,
    pub recent: Vec<FoundFile>,
}

#[tauri::command]
pub fn scan_progress(st: State<AppState>) -> ScanStatus {
    let guard = st.scan.lock().unwrap();
    match guard.as_ref() {
        None => ScanStatus {
            session_id: None,
            running: false,
            progress: ProgressSnapshot::default(),
            error: None,
            recent: vec![],
        },
        Some(s) => {
            let done = s.done.lock().unwrap().clone();
            let running = done.is_none();
            if !running {
                st.locked_sources
                    .lock()
                    .unwrap()
                    .retain(|p| p != &s.source_path);
            }
            ScanStatus {
                session_id: Some(s.session_id.clone()),
                running,
                progress: s.progress.snapshot(),
                error: done.and_then(|d| d.err()),
                recent: vec![],
            }
        }
    }
}

#[tauri::command]
pub fn pause_scan(st: State<AppState>, paused: bool) -> R<()> {
    let g = st.scan.lock().unwrap();
    let s = g.as_ref().ok_or("没有进行中的扫描")?;
    s.progress.set_paused(paused);
    Ok(())
}

#[tauri::command]
pub fn cancel_scan(st: State<AppState>) -> R<()> {
    let g = st.scan.lock().unwrap();
    let s = g.as_ref().ok_or("没有进行中的扫描")?;
    s.progress.cancel();
    Ok(())
}

// ---------- sessions ----------

#[tauri::command]
pub fn list_sessions(st: State<AppState>) -> R<Vec<SessionMeta>> {
    st.store.list().map_err(err)
}

#[tauri::command]
pub fn load_session(st: State<AppState>, id: String) -> R<SessionMeta> {
    st.meta(&id)
}

#[derive(Serialize)]
pub struct Page {
    pub total: usize,
    pub files: Vec<FoundFile>,
    pub selected_bytes: u64,
}

#[tauri::command]
pub fn query_session(st: State<AppState>, id: String, filter: ResultFilter) -> R<Page> {
    let files = st.files_of(&id)?;
    let (page, total) = recovery_core::session::query(&files, &filter);
    Ok(Page {
        total,
        files: page.into_iter().cloned().collect(),
        selected_bytes: 0,
    })
}

#[tauri::command]
pub fn session_tree(st: State<AppState>, id: String) -> R<TreeNode> {
    let files = st.files_of(&id)?;
    Ok(recovery_core::session::build_tree(&files))
}

#[tauri::command]
pub fn session_types(
    st: State<AppState>,
    id: String,
) -> R<std::collections::BTreeMap<String, u64>> {
    let files = st.files_of(&id)?;
    Ok(recovery_core::session::type_counts(&files))
}

#[tauri::command]
pub fn delete_session(st: State<AppState>, id: String) -> R<()> {
    st.cache.lock().unwrap().remove(&id);
    st.store.delete(&id).map_err(err)
}

#[tauri::command]
pub fn export_csv(st: State<AppState>, id: String, path: String) -> R<usize> {
    let files = st.files_of(&id)?;
    std::fs::write(&path, recovery_core::session::export_csv(&files)).map_err(err)?;
    Ok(files.len())
}

#[tauri::command]
pub fn sessions_bytes(st: State<AppState>) -> u64 {
    st.store.total_bytes()
}

// ---------- preview ----------

#[derive(Serialize)]
pub struct Preview {
    pub kind: String, // image | text | hex
    pub mime: Option<String>,
    pub data_base64: Option<String>,
    pub text: Option<String>,
    pub hex: String,
}

#[tauri::command]
pub fn preview_file(st: State<AppState>, id: String, file_id: u64) -> R<Preview> {
    let meta = st.meta(&id)?;
    let files = st.files_of(&id)?;
    let f = files.iter().find(|f| f.id == file_id).ok_or("文件不存在")?;
    let dev = st.device(&meta.device.path)?;
    let vol =
        recovery_core::device::SliceDevice::new(dev, meta.volume_offset, meta.volume_len, "vol")
            .map_err(err)?;
    let head = recovery_core::recover::peek(&vol, f, 64);
    let hex = head
        .chunks(16)
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{:04X}  {:<48} {}",
                i * 16,
                c.iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(" "),
                c.iter()
                    .map(|b| if (0x20..0x7F).contains(b) {
                        *b as char
                    } else {
                        '.'
                    })
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let image_mime = match f.ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    };
    if let Some(mime) = image_mime {
        if f.size <= 24 << 20 {
            use base64::Engine;
            let data = recovery_core::recover::peek(&vol, f, f.size as usize);
            return Ok(Preview {
                kind: "image".into(),
                mime: Some(mime.into()),
                data_base64: Some(base64::engine::general_purpose::STANDARD.encode(data)),
                text: None,
                hex,
            });
        }
    }
    if matches!(
        f.ext.as_str(),
        "txt"
            | "md"
            | "csv"
            | "json"
            | "xml"
            | "html"
            | "log"
            | "ini"
            | "toml"
            | "yaml"
            | "yml"
            | "rtf"
            | "svg"
    ) {
        let data = recovery_core::recover::peek(&vol, f, 16 << 10);
        return Ok(Preview {
            kind: "text".into(),
            mime: None,
            data_base64: None,
            text: Some(String::from_utf8_lossy(&data).to_string()),
            hex,
        });
    }
    Ok(Preview {
        kind: "hex".into(),
        mime: None,
        data_base64: None,
        text: None,
        hex,
    })
}

// ---------- recover ----------

#[derive(Serialize)]
pub struct DestCheck {
    pub ok: bool,
    pub same_disk: bool,
    pub free: u64,
    pub needed: u64,
    pub message: String,
}

#[tauri::command]
pub fn check_destination(
    st: State<AppState>,
    id: String,
    dest: String,
    file_ids: Vec<u64>,
) -> R<DestCheck> {
    let meta = st.meta(&id)?;
    let files = st.files_of(&id)?;
    let needed: u64 = files
        .iter()
        .filter(|f| file_ids.contains(&f.id))
        .map(|f| f.size)
        .sum();
    let d = PathBuf::from(&dest);
    let probe = if d.exists() {
        d.clone()
    } else {
        d.ancestors()
            .find(|a| a.exists())
            .map(|a| a.to_path_buf())
            .unwrap_or(d.clone())
    };
    let same = recovery_platform::same_disk(st.platform.as_ref(), &meta.device.path, &probe);
    let free = recovery_core::recover::free_space(&probe).unwrap_or(0);
    let (ok, message) = if same {
        (
            false,
            "目标与源在同一磁盘上，写入会覆盖尚未恢复的数据".into(),
        )
    } else if free < needed {
        (
            false,
            format!(
                "目标空间不足：需要 {}，可用 {}",
                recovery_core::util::human_bytes(needed),
                recovery_core::util::human_bytes(free)
            ),
        )
    } else {
        (
            true,
            format!(
                "可用空间 {}，预计占用 {}",
                recovery_core::util::human_bytes(free),
                recovery_core::util::human_bytes(needed)
            ),
        )
    };
    Ok(DestCheck {
        ok,
        same_disk: same,
        free,
        needed,
        message,
    })
}

#[derive(Deserialize)]
pub struct StartRecoverArgs {
    pub id: String,
    pub file_ids: Vec<u64>,
    pub options: RecoverOptions,
    pub source_override: Option<String>,
}

#[tauri::command]
pub fn start_recover(st: State<AppState>, args: StartRecoverArgs) -> R<()> {
    if st
        .recover
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.result.lock().unwrap().is_none())
        .unwrap_or(false)
    {
        return Err("已有恢复任务进行中".into());
    }
    let meta = st.meta(&args.id)?;
    let src = args.source_override.unwrap_or(meta.device.path.clone());
    if st.locked_sources.lock().unwrap().contains(&src)
        && st
            .scan
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.done.lock().unwrap().is_none())
            .unwrap_or(false)
    {
        return Err("该卷正在扫描中，恢复操作已锁定".into());
    }
    if recovery_platform::same_disk(st.platform.as_ref(), &src, &args.options.dest) {
        return Err(recovery_core::Error::SameDisk.to_string());
    }
    let files = st.files_of(&args.id)?;
    let selected: Vec<FoundFile> = files
        .iter()
        .filter(|f| args.file_ids.contains(&f.id))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err("没有选择文件".into());
    }
    let dev = st.device(&src)?;
    let vol = Arc::new(
        recovery_core::device::SliceDevice::new(dev, meta.volume_offset, meta.volume_len, "vol")
            .map_err(err)?,
    );
    let progress = AppState::new_progress();
    let result = Arc::new(Mutex::new(None));
    let partial = Arc::new(Mutex::new(vec![]));
    *st.recover.lock().unwrap() = Some(ActiveRecover {
        progress: Arc::clone(&progress),
        result: Arc::clone(&result),
        partial: Arc::clone(&partial),
    });
    let store_root = st.store.root().to_path_buf();
    let opts = args.options;
    let mut meta2 = meta;
    std::thread::Builder::new()
        .name("recover".into())
        .spawn(move || {
            let r =
                recovery_core::recover::recover(vol.as_ref(), &src, &selected, &opts, &progress);
            if let Ok(rep) = &r {
                meta2.recovered += rep.ok + rep.warned;
                let store = recovery_core::session::SessionStore::new(store_root);
                let _ = store.write_meta(&meta2);
                *partial.lock().unwrap() = rep.items.clone();
            }
            *result.lock().unwrap() = Some(r.map_err(|e| e.to_string()));
        })
        .map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
pub struct RecoverStatus {
    pub running: bool,
    pub progress: ProgressSnapshot,
    pub report: Option<RecoverReport>,
    pub error: Option<String>,
}

#[tauri::command]
pub fn recover_progress(st: State<AppState>) -> RecoverStatus {
    let g = st.recover.lock().unwrap();
    match g.as_ref() {
        None => RecoverStatus {
            running: false,
            progress: ProgressSnapshot::default(),
            report: None,
            error: None,
        },
        Some(r) => {
            let res = r.result.lock().unwrap().clone();
            RecoverStatus {
                running: res.is_none(),
                progress: r.progress.snapshot(),
                report: res.as_ref().and_then(|x| x.as_ref().ok().cloned()),
                error: res.and_then(|x| x.err()),
            }
        }
    }
}

// ---------- image ----------

#[tauri::command]
pub fn create_image(st: State<AppState>, source: String, dest: String) -> R<()> {
    if st
        .recover
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.result.lock().unwrap().is_none())
        .unwrap_or(false)
    {
        return Err("已有任务进行中".into());
    }
    let dev = st.device(&source)?;
    let d = PathBuf::from(&dest);
    if recovery_platform::same_disk(st.platform.as_ref(), &source, d.parent().unwrap_or(&d)) {
        return Err(recovery_core::Error::SameDisk.to_string());
    }
    let progress = AppState::new_progress();
    let result = Arc::new(Mutex::new(None));
    *st.recover.lock().unwrap() = Some(ActiveRecover {
        progress: Arc::clone(&progress),
        result: Arc::clone(&result),
        partial: Arc::new(Mutex::new(vec![])),
    });
    std::thread::spawn(move || {
        let r = recovery_core::image::create_image(dev.as_ref(), &source, &d, &progress);
        *result.lock().unwrap() = Some(
            r.map(|_| RecoverReport::default())
                .map_err(|e| e.to_string()),
        );
    });
    Ok(())
}

// ---------- signatures ----------

#[tauri::command]
pub fn list_signatures(st: State<AppState>) -> Vec<Signature> {
    st.sigs.lock().unwrap().signatures.clone()
}

fn save_sigs(st: &AppState) -> R<()> {
    st.sigs.lock().unwrap().save(&st.sig_path).map_err(err)
}

#[tauri::command]
pub fn upsert_signature(st: State<AppState>, sig: Signature) -> R<()> {
    st.sigs.lock().unwrap().upsert(sig).map_err(err)?;
    save_sigs(&st)
}

#[tauri::command]
pub fn set_signature_enabled(st: State<AppState>, id: String, enabled: bool) -> R<()> {
    st.sigs
        .lock()
        .unwrap()
        .set_enabled(&id, enabled)
        .map_err(err)?;
    save_sigs(&st)
}

#[tauri::command]
pub fn delete_signature(st: State<AppState>, id: String) -> R<()> {
    st.sigs.lock().unwrap().delete(&id).map_err(err)?;
    save_sigs(&st)
}

#[tauri::command]
pub fn restore_signature_defaults(st: State<AppState>) -> R<()> {
    st.sigs.lock().unwrap().restore_defaults();
    save_sigs(&st)
}

#[tauri::command]
pub fn export_signatures(st: State<AppState>, path: String) -> R<()> {
    std::fs::write(path, st.sigs.lock().unwrap().export_toml()).map_err(err)
}

#[tauri::command]
pub fn import_signatures(st: State<AppState>, path: String) -> R<usize> {
    let txt = std::fs::read_to_string(path).map_err(err)?;
    let n = st.sigs.lock().unwrap().import_toml(&txt).map_err(err)?;
    save_sigs(&st)?;
    Ok(n)
}

/// Test a signature against a sample file: does the header match at offset 0 and what length does the resolver produce?
#[tauri::command]
pub fn test_signature(sig: Signature, sample: String) -> R<serde_json::Value> {
    sig.validate().map_err(err)?;
    let dev = recovery_core::device::FileDevice::open(Path::new(&sample)).map_err(err)?;
    let probe = dev
        .read_vec(0, (dev.size() as usize).min(1 << 20))
        .map_err(err)?;
    let matched = sig.headers.iter().any(|h| h.matches(&probe));
    let len = if matched {
        recovery_core::carve::resolve::resolve_len(&dev, &sig, 0, &probe, probe.len()).len
    } else {
        None
    };
    Ok(
        serde_json::json!({"matched": matched, "resolved_len": len, "actual_len": dev.size(), "exact": len == Some(dev.size())}),
    )
}

// ---------- diagnostics ----------

#[tauri::command]
pub fn diagnostics(st: State<AppState>, app: tauri::AppHandle) -> serde_json::Value {
    let p = st.platform.privilege_state();
    serde_json::json!({
        "app_version": app.package_info().version.to_string(),
        "core_version": env!("CARGO_PKG_VERSION"),
        "signatures": st.sigs.lock().unwrap().signatures.len(),
        "signatures_enabled": st.sigs.lock().unwrap().enabled().len(),
        "privilege": p,
        "data_dir": st.data_dir.display().to_string(),
        "sessions_bytes": st.store.total_bytes(),
        "webview": tauri::webview_version().unwrap_or_default(),
    })
}

#[tauri::command]
pub fn data_dir(st: State<AppState>) -> String {
    st.data_dir.display().to_string()
}
