# FileRescue 文件找回 — 方案与界面说明（v0.1 待确认）

> 状态：**界面原型阶段**，未开始开发。确认本文档与 `docs/ui-mockup/index.html` 后进入骨架阶段。

## 1. 产品定义

| 项 | 内容 |
| --- | --- |
| 核心任务 | 在 Windows / macOS 上找回已删除文件：① 指定目录找回（保留文件名/路径）；② 全盘找回（元数据 + 签名雕刻） |
| 用户 | 普通用户与 IT 支持人员；不要求懂文件系统 |
| 输入 | 一个磁盘/卷（只读打开）+ 可选原目录路径 + 文件类型过滤 |
| 输出 | 恢复到**另一块磁盘**的文件，保留目录结构与时间戳；恢复报告（CSV/JSON） |
| 平台 | Windows 10/11 x64；macOS 12+（Apple Silicon / Intel）。两套原生安装包，共用 Rust 核心与前端 |
| 离线 | 完全离线可安装、可运行；更新检查可联网但非必需 |
| 署名 | M&J Group · 473276@qq.com |
| 语言 | 简中 / 繁中 / 英 / 日 |

验收样例：在 NTFS 数据盘删除 `D:\项目\2025-合同\` 下 50 个文件（含子目录、含 1 个 >100 MB 文件），用“指定目录找回·快速”在 5 分钟内列出 ≥ 48 个且文件名/路径正确，恢复后 SHA256 与原文件一致。

## 2. 开源项目调研与选型

### 2.1 候选

| 项目 | 语言 / 许可证 | 能力 | 结论 |
| --- | --- | --- | --- |
| **ntfs** crate (ColinFinck) | Rust · MIT/Apache-2.0 | 只读 NTFS 解析，遍历 $MFT、属性、数据运行；可读取标记为未使用的记录 | **采用**，作为 NTFS 元数据恢复基础；需补“已删除记录枚举 + 父目录重建”层 |
| **fatfs** crate | Rust · MIT | FAT12/16/32 读写 | **采用**（只读），exFAT 需自写解析器或参考 TSK |
| **apfs** 相关 crate（如 `apfs` / `apfs-fuse` 的 Rust 移植） | Rust · MIT/Apache | 仅部分可用，B-tree/快照解析不完整 | 作为参考；**APFS 元数据解析自写**（容器超级块 → 卷 → fs 树中标记删除的 inode / 对象历史） |
| **The Sleuth Kit (TSK)** | C · IPL/CPL/GPL 混合 | 支持 NTFS/FAT/exFAT/ext/HFS+/APFS；`fls -d` 列出已删除文件、`tsk_recover` 恢复 | **可选 sidecar**：第一版用于 **HFS+/exFAT** 及 APFS 回退路径，通过子进程调用，不链接到 Rust 二进制，避免许可证传染 |
| **PhotoRec / TestDisk** | C · GPLv2+ | 400+ 文件签名雕刻；成熟 | **不捆绑二进制**。参考其签名规则（header/footer/最大大小/格式内部校验）在 Rust 中自实现签名库；签名数据本身是事实不受版权约束，实现代码自写 |
| **Scalpel / foremost** | C · GPL/公有领域 | 纯雕刻 | 参考 foremost（公有领域）的配置文件格式作为签名库 TOML 的设计来源 |
| **RecuperaBit** | Python · GPLv3 | NTFS 目录结构重建算法 | 参考其“从孤立 MFT 记录重建目录树”思路，不复用代码 |
| **ddrescue** | C · GPL | 坏扇区容错读取 | 参考其读取策略（跳过/回头/重试），Rust 自实现 |

### 2.2 结论

- **核心全部 Rust 自有实现**（许可证干净：MIT/Apache），可以商业分发。
- TSK 作为**可选 sidecar**（单独进程、单独许可证声明、随包附带对应源码链接），仅覆盖第一版不自写的文件系统；后续逐步替换。
- 不捆绑 PhotoRec/TestDisk 二进制。

## 3. 架构

```
filerescue/
├─ crates/
│  ├─ recovery-core/      # 纯逻辑，无 GUI 依赖，可单测
│  │   ├─ fs/ntfs         # 基于 ntfs crate：已删除记录枚举、目录重建、数据运行提取、覆盖率评估
│  │   ├─ fs/fat          # FAT/exFAT 已删除目录项 (0xE5)
│  │   ├─ fs/apfs         # APFS 容器/卷/快照/删除 inode
│  │   ├─ carve/          # 签名库 (TOML) + 多线程扇区扫描 + 格式校验 (ZIP 目录/JPEG 段/PDF xref)
│  │   ├─ session/        # 扫描会话：进度、暂停/继续、结果持久化 (SQLite)
│  │   └─ recover/        # 写出、同盘拒绝、原子写、校验、时间戳
│  ├─ recovery-platform/  # 设备枚举与只读打开
│  │   ├─ windows.rs      # \\.\PhysicalDriveN / \\.\C: 、BitLocker、TRIM 检测、锁定检测
│  │   └─ macos.rs        # /dev/rdiskN、diskutil list -plist、FileVault、TCC 状态
│  └─ recovery-cli/       # 同一核心的命令行（无人值守、测试用）
├─ src-tauri/             # Tauri 2：IPC、权限、助手、托盘、updater
├─ src/                   # Vue 3 + TypeScript 前端
├─ helper/                # macOS 特权助手（SMAppService，仅只读打开设备）
└─ docs/
```

### 3.1 平台边界（关键差异）

| 事项 | Windows | macOS |
| --- | --- | --- |
| 提权 | UAC 自提升（manifest `requireAdministrator` 或“以管理员重启”） | 特权助手（SMAppService / launchd），GUI 保持普通用户 |
| 原始设备 | `\\.\PhysicalDriveN`、`\\.\X:`，`FILE_SHARE_READ\|WRITE` 只读 | `/dev/rdiskN`，需要 root；系统盘还需 **完全磁盘访问（TCC）** |
| 加密 | BitLocker 已解锁卷通过卷路径读明文 | FileVault 内置卷原始读为密文 → **雕刻不可用**；走 APFS 元数据 + 本地快照 |
| SSD/TRIM | `fsutil behavior query DisableDeleteNotify` 提示成功率 | 内置 NVMe 默认 TRIM；界面提示 |
| 文件系统 | NTFS（主）、FAT/exFAT、（可读 APFS/HFS+ 外置盘） | APFS（主）、HFS+、FAT/exFAT、NTFS（外置盘） |

### 3.2 能力矩阵（第一版目标）

| 文件系统 | 指定目录找回 | 全盘·元数据 | 全盘·雕刻 | 实现 |
| --- | --- | --- | --- | --- |
| NTFS | ✅ | ✅ | ✅ | Rust 自有 |
| FAT32 / exFAT | ✅（短/长文件名） | ✅ | ✅ | Rust 自有（exFAT 自写） |
| APFS 未加密 | ✅ | ✅ | ✅ | Rust 自有 |
| APFS FileVault | ⚠ 元数据 + 本地快照 | ⚠ | ❌ 密文 | Rust 自有 |
| HFS+ | ⚠ 第一版经 TSK sidecar | ⚠ | ✅ | TSK → 后续替换 |
| 分区表损坏 / 未知 | ❌ | ❌ | ✅ 全盘雕刻 | Rust 自有 |

## 4. 界面结构（对应 `docs/ui-mockup/index.html`）

左侧导航是 4 步流程 + 辅助页，顶栏显示模式、权限状态、平台、语言、主题。

| 页面 | 要点 |
| --- | --- |
| 1 选择模式与来源 | 两个模式卡片（指定目录 / 全盘），磁盘列表（文件系统、容量、健康、支持能力），右侧扫描设置（目录路径、快速/深度、文件类型、只读镜像开关） ；SSD/TRIM、FileVault 风险提示 |
| 2 扫描 | 四阶段进度（引导扇区 → MFT/元数据 → 雕刻 → 整理）、速度/剩余时间/读错误、实时发现、日志；暂停 / 停止并查看 / 后台；扫描期间锁定该卷的恢复操作 |
| 3 结果与预览 | 三栏：目录树（按原目录 / 类型 / 删除时间）+ 可筛选列表（可恢复性 高/中/低、大小、时间、来源 MFT/签名）+ 预览（缩略图/文本/十六进制、簇覆盖率）。“低”默认不勾选 |
| 4 恢复 | 目标路径（Rust 端拒绝同盘）、保留目录结构、同名策略、恢复后格式校验、时间戳；逐文件进度与结果；导出报告 |
| 扫描记录 | 会话自动保存（SQLite），可重开 / 继续 / 删除 |
| 设置 | 通用（语言/主题/数据目录/关闭行为）、**签名库在界面内增删改/启停/导入导出/恢复默认**、扫描与性能、更新（含大陆镜像） |
| 权限向导 | Windows：管理员 / BitLocker / 写入进程提醒；macOS：助手授权 / 完全磁盘访问 / FileVault 说明 |
| 诊断与关于 | 组件版本一致性、WebView、签名库；脱敏诊断导出；开源组件许可证 |

原型支持：浅/深色切换、Windows/macOS 模拟切换（磁盘列表与权限向导随平台变化）、窄屏折叠侧栏、模态 Escape 关闭。

## 5. 需要你确认的问题

1. **产品名**：暂定 `FileRescue 文件找回`，Bundle ID `com.mjgroup.filerescue`。是否沿用？
2. **HFS+ 是否必须第一版支持**？若否，可去掉 TSK sidecar，第一版完全无 GPL 组件。
3. **只读镜像功能**（扫描前把整个卷镜像到另一块盘再扫）是否第一版需要？
4. **是否需要 CLI**（无人值守 / 脚本调用）？建议保留，用于自动化测试。
5. 界面布局、术语、流程步骤是否符合预期？有无需要增删的页面或字段？

## 6. 确认后的开发顺序

| 阶段 | 可检查结果 |
| --- | --- |
| 骨架 | Tauri 2 + Vue 3 工程、workspace、设备枚举（Win/mac）、只读打开、权限向导可用 |
| 核心任务 | NTFS 指定目录找回全链路：枚举已删除 MFT 记录 → 目录重建 → 提取 → 校验；CLI 与 GUI 共用 |
| 全盘 | 签名库 + 多线程雕刻 + 格式校验；FAT/exFAT；APFS |
| 本地可靠性 | 会话持久化、暂停/继续、坏扇区策略、同盘拒绝、原子写 |
| 扩展体验 | 四语、主题、托盘、预览、CSV 报告 |
| 包与更新 | NSIS/MSI（内置 WebView2 离线安装器）、DMG/PKG（含助手）、signed updater、GH-Proxy 镜像 |
