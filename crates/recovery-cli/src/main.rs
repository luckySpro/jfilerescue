//! `jfr` — JFileRescue CLI. Shares the exact same core as the GUI so scans are
//! scriptable and testable without a window.

use clap::{Parser, Subcommand, ValueEnum};
use recovery_core::carve::signatures::SignatureLibrary;
use recovery_core::model::{ScanDepth, ScanMode, ScanRequest};
use recovery_core::progress::{Phase, Progress};
use recovery_core::recover::{Conflict, RecoverOptions};
use recovery_core::session::{DeviceRef, SessionState, SessionStore};
use recovery_core::util::human_bytes;
use recovery_platform::open_source;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "jfr",
    version,
    about = "JFileRescue — 只读文件找回工具 (M&J Group)"
)]
struct Cli {
    /// Output JSON instead of human text.
    #[arg(long, global = true)]
    json: bool,
    /// Data directory (sessions + signature overrides). Default: platform app data dir.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Directory,
    Full,
}
#[derive(Clone, Copy, ValueEnum)]
enum DepthArg {
    Quick,
    Deep,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show privilege state and list disks / volumes.
    Devices,
    /// Scan a device or image file for deleted files.
    Scan {
        /// Device path (\\.\PhysicalDrive1, \\.\D:, /dev/rdisk4) or image file.
        source: String,
        #[arg(long, value_enum, default_value = "full")]
        mode: ModeArg,
        #[arg(long, value_enum, default_value = "quick")]
        depth: DepthArg,
        /// Original directory (for --mode directory), e.g. "项目/2025-合同" or "D:\项目\2025-合同".
        #[arg(long)]
        dir: Option<String>,
        /// Partition index inside a disk (default: first recognized).
        #[arg(long)]
        partition: Option<u32>,
        /// Restrict to extensions, comma separated (jpg,docx).
        #[arg(long)]
        ext: Option<String>,
        /// Carve the whole volume instead of only unallocated clusters.
        #[arg(long)]
        carve_all: bool,
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// Do not persist a session.
        #[arg(long)]
        no_save: bool,
    },
    /// List saved sessions.
    Sessions,
    /// Show files of a session (optionally filtered by query).
    Show {
        session: String,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Export CSV to this path instead of printing.
        #[arg(long)]
        csv: Option<PathBuf>,
    },
    /// Recover files from a session to a destination on another disk.
    Recover {
        session: String,
        /// Destination directory.
        dest: PathBuf,
        /// File ids (comma separated). Default: all with quality >= medium.
        #[arg(long)]
        ids: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        flat: bool,
        #[arg(long, value_enum, default_value = "rename")]
        conflict: ConflictArg,
        /// Override the source device path (e.g. recover from an image instead).
        #[arg(long)]
        source: Option<String>,
    },
    /// Create a read-only sector image of a device.
    Image { source: String, dest: PathBuf },
    /// Signature library management.
    Sigs {
        #[command(subcommand)]
        cmd: SigCmd,
    },
    /// Delete a session.
    Delete { session: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ConflictArg {
    Rename,
    Skip,
    Overwrite,
}

#[derive(Subcommand)]
enum SigCmd {
    List,
    Enable { id: String },
    Disable { id: String },
    Export { path: PathBuf },
    Import { path: PathBuf },
    RestoreDefaults,
}

fn data_dir(cli: &Cli) -> PathBuf {
    cli.data_dir.clone().unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("com.mjgroup.jfilerescue")
    })
}

fn main() {
    let cli = Cli::parse();
    let code = match run(&cli) {
        Ok(()) => 0,
        Err(e) => {
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"error": e.to_string(), "code": e.code()})
                );
            } else {
                eprintln!("错误: {}", e);
            }
            match e {
                recovery_core::Error::AccessDenied(_) => 3,
                recovery_core::Error::SameDisk => 4,
                recovery_core::Error::Cancelled => 130,
                _ => 1,
            }
        }
    };
    std::process::exit(code);
}

fn run(cli: &Cli) -> recovery_core::Result<()> {
    let platform = recovery_platform::current();
    let dd = data_dir(cli);
    let store = SessionStore::new(dd.join("sessions"));
    let sig_path = dd.join("signatures.toml");
    let sigs = SignatureLibrary::load(&sig_path)?;

    match &cli.cmd {
        Cmd::Devices => {
            let priv_state = platform.privilege_state();
            let disks = platform.list_disks()?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"privilege": priv_state, "disks": disks})
                    )
                    .unwrap()
                );
                return Ok(());
            }
            println!(
                "权限: {}  ({} {} {})",
                if priv_state.elevated {
                    "已提权"
                } else {
                    "未提权"
                },
                priv_state.os,
                priv_state.arch,
                priv_state.os_version
            );
            for d in &priv_state.details {
                println!("  {}", d);
            }
            for d in disks {
                println!(
                    "\n{}  {}  {}  {}{}",
                    d.path,
                    d.display_name,
                    human_bytes(d.size),
                    d.health_text,
                    match d.is_ssd {
                        Some(true) => " · SSD",
                        Some(false) => " · HDD",
                        None => "",
                    }
                );
                if d.trim_enabled == Some(true) {
                    println!("  ⚠ TRIM 已启用，被删除数据可能已被擦除");
                }
                for v in d.volumes {
                    println!(
                        "  {}  {}  {}  {}{}",
                        v.path,
                        v.display_name,
                        v.fs_label,
                        human_bytes(v.size),
                        if v.encrypted { "  [加密]" } else { "" }
                    );
                    for n in v.notes {
                        println!("      {}", n);
                    }
                }
            }
        }
        Cmd::Scan {
            source,
            mode,
            depth,
            dir,
            partition,
            ext,
            carve_all,
            threads,
            no_save,
        } => {
            let dev = open_source(platform.as_ref(), source)?;
            let req = ScanRequest {
                mode: match mode {
                    ModeArg::Directory => ScanMode::Directory,
                    ModeArg::Full => ScanMode::Full,
                },
                depth: match depth {
                    DepthArg::Quick => ScanDepth::Quick,
                    DepthArg::Deep => ScanDepth::Deep,
                },
                directory: dir.clone(),
                extensions: ext
                    .as_deref()
                    .map(|e| {
                        e.split(',')
                            .map(|s| s.trim().trim_start_matches('.').to_lowercase())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
                include_damaged_names: true,
                carve_unallocated_only: !carve_all,
                threads: *threads,
            };
            if req.mode == ScanMode::Directory && req.directory.is_none() {
                return Err(recovery_core::Error::InvalidInput(
                    "--mode directory 需要 --dir".into(),
                ));
            }
            let progress = Progress::new();
            let dev_ref = DeviceRef {
                path: source.clone(),
                identity: platform
                    .disk_identity_of_device(source)
                    .unwrap_or_else(|| format!("{}:{}", source, dev.size())),
                size: dev.size(),
                sector_size: dev.sector_size(),
                partition: *partition,
                display_name: dev.describe(),
                is_image: std::path::Path::new(source).is_file(),
            };
            let mut meta = if *no_save {
                None
            } else {
                Some(store.create(dev_ref, req.clone())?)
            };

            // Ctrl-C → cancel.
            {
                let p = Arc::clone(&progress);
                let _ = ctrlc_handler(move || p.cancel());
            }
            let p2 = Arc::clone(&progress);
            let json = cli.json;
            let reporter = std::thread::spawn(move || {
                let start = Instant::now();
                let mut last = Phase::Idle;
                loop {
                    let s = p2.snapshot();
                    if matches!(s.phase, Phase::Done | Phase::Cancelled | Phase::Failed) {
                        break;
                    }
                    if !json {
                        if s.phase != last {
                            eprintln!();
                            last = s.phase;
                        }
                        let pct = if s.total > 0 {
                            s.done * 100 / s.total
                        } else {
                            0
                        };
                        eprint!(
                            "\r[{:?}] {} {}%  已发现 {}  已读 {}  读错误 {}  {}s   ",
                            s.phase,
                            s.message,
                            pct,
                            s.found,
                            human_bytes(s.bytes_read),
                            s.read_errors,
                            start.elapsed().as_secs()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                if !json {
                    eprintln!();
                }
            });
            let res = recovery_core::scan::run_scan(dev, *partition, &req, &sigs, &progress);
            match &res {
                Ok(_) => progress.set_phase(Phase::Done, 1),
                Err(recovery_core::Error::Cancelled) => progress.set_phase(Phase::Cancelled, 1),
                Err(_) => progress.set_phase(Phase::Failed, 1),
            }
            let _ = reporter.join();
            match res {
                Ok(outcome) => {
                    if let Some(m) = meta.as_mut() {
                        store.complete(m, &outcome, SessionState::Completed)?;
                    }
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"session": meta.as_ref().map(|m| m.id.clone()), "outcome": outcome})).unwrap());
                    } else {
                        if let Some(w) = &outcome.warning {
                            println!("⚠ {}", w);
                        }
                        println!(
                            "文件系统: {}  卷标: {}",
                            outcome.fs.label(),
                            outcome.volume_label.clone().unwrap_or_default()
                        );
                        let s = &outcome.summary;
                        println!(
                            "共 {} 个文件 ({})，含文件名 {}，签名雕刻 {}，高/中/低 = {}/{}/{}",
                            s.total,
                            human_bytes(s.bytes),
                            s.with_name,
                            s.carved,
                            s.high,
                            s.medium,
                            s.low
                        );
                        for f in outcome.files.iter().take(30) {
                            println!(
                                "  #{:<6} {:<6?} {:>10}  {}",
                                f.id,
                                f.quality,
                                human_bytes(f.size),
                                f.full_path()
                            );
                        }
                        if outcome.files.len() > 30 {
                            println!(
                                "  … 另有 {} 个，使用 `jfr show <session>` 查看",
                                outcome.files.len() - 30
                            );
                        }
                        if let Some(m) = &meta {
                            println!("会话已保存: {}", m.id);
                        }
                    }
                }
                Err(e) => {
                    if let Some(m) = meta.as_mut() {
                        store.fail(m, &e.to_string())?;
                    }
                    return Err(e);
                }
            }
        }
        Cmd::Sessions => {
            let list = store.list()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&list).unwrap());
            } else {
                for m in list {
                    println!(
                        "{}  {:?}  {}  {}  发现 {}  已恢复 {}",
                        m.id, m.state, m.device.display_name, m.fs, m.summary.total, m.recovered
                    );
                }
            }
        }
        Cmd::Show {
            session,
            query,
            limit,
            csv,
        } => {
            let s = store.load(session)?;
            if let Some(p) = csv {
                std::fs::write(p, recovery_core::session::export_csv(&s.files))?;
                println!("已导出 {} 条到 {}", s.files.len(), p.display());
                return Ok(());
            }
            let filter = recovery_core::model::ResultFilter {
                query: query.clone().unwrap_or_default(),
                limit: *limit,
                ..Default::default()
            };
            let (page, total) = recovery_core::session::query(&s.files, &filter);
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"total": total, "files": page})
                    )
                    .unwrap()
                );
            } else {
                for f in page {
                    println!(
                        "#{:<6} {:<6?} {:>3}% {:>10}  {}  {}",
                        f.id,
                        f.quality,
                        f.overwritten_pct,
                        human_bytes(f.size),
                        f.full_path(),
                        f.note
                    );
                }
                println!("匹配 {} 条", total);
            }
        }
        Cmd::Recover {
            session,
            dest,
            ids,
            all,
            flat,
            conflict,
            source,
        } => {
            let s = store.load(session)?;
            let src = source.clone().unwrap_or(s.meta.device.path.clone());
            let dev = open_source(platform.as_ref(), &src)?;
            if recovery_platform::same_disk(platform.as_ref(), &src, dest) {
                return Err(recovery_core::Error::SameDisk);
            }
            let selected: Vec<_> = match ids {
                Some(list) => {
                    let want: Vec<u64> = list
                        .split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                    s.files
                        .iter()
                        .filter(|f| want.contains(&f.id))
                        .cloned()
                        .collect()
                }
                None if *all => s.files.clone(),
                None => s
                    .files
                    .iter()
                    .filter(|f| f.quality >= recovery_core::model::Quality::Medium)
                    .cloned()
                    .collect(),
            };
            let opts = RecoverOptions {
                dest: dest.clone(),
                keep_structure: !flat,
                conflict: match conflict {
                    ConflictArg::Rename => Conflict::Rename,
                    ConflictArg::Skip => Conflict::Skip,
                    ConflictArg::Overwrite => Conflict::Overwrite,
                },
                verify: true,
                restore_times: true,
            };
            let progress = Progress::new();
            let rep =
                recovery_core::recover::recover(dev.as_ref(), &src, &selected, &opts, &progress)?;
            let mut meta = s.meta.clone();
            meta.recovered += rep.ok + rep.warned;
            store.write_meta(&meta)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&rep).unwrap());
            } else {
                for i in &rep.items {
                    println!("{:?}  {}  {}", i.status, i.name, i.message);
                }
                println!(
                    "成功 {}  带警告 {}  跳过 {}  失败 {}  共 {}",
                    rep.ok,
                    rep.warned,
                    rep.skipped,
                    rep.failed,
                    human_bytes(rep.bytes)
                );
            }
        }
        Cmd::Image { source, dest } => {
            let dev = open_source(platform.as_ref(), source)?;
            if recovery_platform::same_disk(
                platform.as_ref(),
                source,
                dest.parent().unwrap_or(dest),
            ) {
                return Err(recovery_core::Error::SameDisk);
            }
            let progress = Progress::new();
            let p2 = Arc::clone(&progress);
            let total = dev.size();
            let t = std::thread::spawn(move || loop {
                let s = p2.snapshot();
                if matches!(s.phase, Phase::Done | Phase::Failed | Phase::Cancelled) {
                    break;
                }
                eprint!(
                    "\r镜像 {} / {}  坏扇区 {}   ",
                    human_bytes(s.done),
                    human_bytes(total),
                    s.read_errors
                );
                std::thread::sleep(Duration::from_millis(500));
            });
            let r = recovery_core::image::create_image(dev.as_ref(), source, dest, &progress);
            progress.set_phase(
                if r.is_ok() {
                    Phase::Done
                } else {
                    Phase::Failed
                },
                1,
            );
            let _ = t.join();
            eprintln!();
            let rep = r?;
            println!(
                "镜像完成: {} ({}), 坏扇区 {}",
                rep.path,
                human_bytes(rep.bytes),
                rep.bad_sectors
            );
        }
        Cmd::Sigs { cmd } => {
            let mut lib = sigs;
            match cmd {
                SigCmd::List => {
                    for s in &lib.signatures {
                        println!(
                            "{} {:<14} {:<30} {:<24} {}",
                            if s.enabled { "✓" } else { "✗" },
                            s.id,
                            s.name,
                            s.extensions.join("/"),
                            if s.builtin { "内置" } else { "自定义" }
                        );
                    }
                }
                SigCmd::Enable { id } => {
                    lib.set_enabled(id, true)?;
                    lib.save(&sig_path)?;
                }
                SigCmd::Disable { id } => {
                    lib.set_enabled(id, false)?;
                    lib.save(&sig_path)?;
                }
                SigCmd::Export { path } => std::fs::write(path, lib.export_toml())?,
                SigCmd::Import { path } => {
                    let n = lib.import_toml(&std::fs::read_to_string(path)?)?;
                    lib.save(&sig_path)?;
                    println!("导入 {} 条自定义签名", n);
                }
                SigCmd::RestoreDefaults => {
                    lib.restore_defaults();
                    lib.save(&sig_path)?;
                }
            }
        }
        Cmd::Delete { session } => store.delete(session)?,
    }
    Ok(())
}

/// Minimal Ctrl-C hook without an extra crate.
fn ctrlc_handler<F: Fn() + Send + Sync + 'static>(f: F) -> std::io::Result<()> {
    static HANDLER: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();
    let _ = HANDLER.set(Box::new(f));
    #[cfg(windows)]
    {
        extern "system" {
            fn SetConsoleCtrlHandler(
                h: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        unsafe extern "system" fn handler(_: u32) -> i32 {
            if let Some(h) = HANDLER.get() {
                h();
            }
            1
        }
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn signal(sig: i32, h: usize) -> usize;
        }
        extern "C" fn handler(_: i32) {
            if let Some(h) = HANDLER.get() {
                h();
            }
        }
        unsafe {
            signal(2, handler as usize);
        }
    }
    Ok(())
}
