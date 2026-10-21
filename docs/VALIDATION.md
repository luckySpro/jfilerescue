# JFileRescue 验证记录

## 0.1.1（2026-10-21）

起因：用户在 `C:\Users\Administrator\Desktop\testjfile` 删除文件后用 0.1.0 找回为 0。根因分析见 git 提交说明与本节；三项根因均已修复并以真实卷复验。

| 项 | 等级 | 结果 |
| --- | --- | --- |
| Rust 单元测试（24，新增 `$I` 记录 v1/v2 解析、`$R`/`$I` 配对、回收站目录识别） | 单元 | ✅ |
| **真实卷集成测试（8）**：`ntfs.img` / `fat32.img` / `exfat.img`（diskpart 生成，TRIM 关闭，含一个经回收站删除并清空的文件）+ `*-trimmed.img`（TRIM 开启时生成，数据簇全零）+ FAT32 内存卷 + APFS（无镜像，跳过） | 真实文件系统 | ✅ 8/8。NTFS：6 个文件按原路径找到，`recycled.pdf` 从 `$Recycle.Bin` 还原为 `项目/2025-合同/recycled.pdf` 且 `via_recycle_bin=true`、删除时间来自 `$I`；数据 SHA256 一致；驻留文件正确。exFAT：全部 High、0% 覆盖。FAT32：8.3 名首字节丢失按 FAT 固有行为接受。TRIM 镜像：条目仍找到，全部标 Low + "可能已被 TRIM 擦除"，outcome 带汇总警告；NTFS 驻留小文件仍 High |
| CLI 端到端（真实 NTFS 镜像）：`scan --mode directory` → `show` → `recover --all` | 真实二进制 | ✅ 6/6 恢复，`IMG_0001.jpg` SHA256 `3fdba6c3…` 与原始一致，`recycled.pdf` 以 `%PDF` 开头 |
| clippy `-D warnings`、vue-tsc、vite build、Playwright 截图（新增风险对话框与"源盘已被写入"警告页） | 静态/构建 | ✅ |
| 生产 NSIS 包 + updater 签名 | 最终包 | ✅ 本地 `JFileRescue_0.1.1_x64-setup.exe` + `.sig`；CI 产出 13 个附件（Windows NSIS/MSI×2、macOS arm64/x64 DMG + app.tar.gz）+ `latest.json`，公开后独立复验：4 个下载 URL 200、`latest.json`（官方与 GH-Proxy 镜像）4 个平台签名字段完整、`scripts/verify-sig.mjs` 密码学验证 3 个 updater 包签名有效 |
| **真实客户端自动更新 0.1.0 → 0.1.1** | 安装/升级冒烟 | ✅ 在本机已安装的 0.1.0（`D:\Program Files\JFileRescue`）上，通过 WebView2 远程调试驱动其**自带**更新逻辑：`check()` 看到 0.1.1 → `downloadAndInstall()` 下载 221 MB → 验签 → 静默 NSIS 安装 → 自动重启；重启后注册表与二进制内容均为 0.1.1。过程中发现 0.1.0 设置页把 `check()` 返回的 Update 资源缓存到后续点击，再次检查/关闭后 rid 失效报 "The resource id … is invalid"；0.1.1 改为安装前重新 `check()`、显示下载进度与安装状态、失败时保留"下载并安装"按钮 |

真实镜像测试中暴露并修复的缺陷（0.1.0 均存在）：
1. `AllocBitmap` 对 FAT/exFAT 使用绝对簇号索引，但位图按数据区相对簇号存储 → 被释放的簇被判"已分配"，评分 Low；修正为带 `base` 偏移的位图。
2. CLI `recover` 用整盘设备读取卷相对 extents → 带分区表的镜像恢复出错误字节；改为按卷切片。
3. NTFS 非驻留属性长度不足 64 字节时越界 panic；空属性 `last_vcn = -1` 加一溢出。
4. 测试镜像生成脚本本身的两个坑（记录以免再犯）：Windows 对 VHD 发 TRIM，被删簇立即清零，需 `fsutil behavior set DisableDeleteNotify 1`；写入后立刻删除的文件数据未落盘，需先 `fsync`。

## 0.1.0（2026-10-20）

版本 0.1.0 · 提交见 `git log` · 记录日期 2026-10-20 · 机器：Windows 11 Pro 10.0.26200 x64，Rust 1.99.1 (stable-msvc)，Node 24.20，pnpm 9.15.4，VS 2022 Build Tools 14.44，WebView2 155。

## 证据等级总览

| 项 | 等级 | 结果 |
| --- | --- | --- |
| Rust 单元测试（21）：NTFS 数据运行/修正序列、FAT 时间、分区表、签名库校验/覆盖/导入导出、PNG/ZIP/JPEG 长度解析、会话存储、恢复写出 | 单元/契约 | ✅ 全部通过 |
| **FAT32 真实卷集成测试**（`fatfs` 在内存中格式化 64 MiB 卷 → 写入 → 删除 → 指定目录扫描 → SHA256 比对 → 恢复到临时目录 → 全盘深度雕刻命中 JPEG） | 真实文件系统（内存镜像） | ✅ 通过：文件名/路径正确、目录过滤不泄漏、数据哈希一致、恢复文件哈希一致、雕刻长度精确 |
| NTFS / exFAT / APFS 真实镜像测试（`test-images/*.img`） | 真实文件系统 | ⏳ **未执行**：镜像生成需要管理员 diskpart，UAC 提示被取消；测试在镜像不存在时自动跳过。见下文"待办" |
| `cargo clippy --workspace --all-targets -D warnings` | 静态检查 | ✅ 0 警告（修复后） |
| 前端 `vue-tsc` + `vite build` | 类型/构建 | ✅ 通过，主包 74 KB gzip |
| Playwright 截图（浅/深色 ×8 页 + 窄屏 ×2）与横向溢出检查 | 模拟界面 | ✅ 生成 `docs/screenshots/*.png`；溢出元素已修复后复检 |
| CLI `jfr --help / sigs list / devices` | 真实二进制 | ✅ 运行；`devices` 在未提权 shell 中正确报告"未提权" |
| **CLI 端到端**（生成的 FAT32 镜像）：`scan --mode directory` → 3 个文件含正确路径 → `show` → `recover --all` 到临时目录 → JPEG 头尾 `FF D8 … FF D9`、长度 200026 与原始一致；docx/xlsx 测试占位内容被校验正确标记"文件头不是 ZIP"（带警告）→ `scan --mode full --depth deep --ext jpg` 雕刻命中同一 JPEG（195.3 KB，High）→ CSV 导出 | 真实二进制 + 真实文件系统镜像 | ✅ 通过 |
| 同盘拒绝 | 契约 | ✅ 对真实卷/物理盘生效（Windows 卷句柄字母、平台层卷→磁盘映射）；镜像文件不视为同盘（已修正，原先误拒） |
| Windows 生产安装包 `pnpm tauri build --bundles nsis` | 最终包 | ✅ `target/release/bundle/nsis/JFileRescue_0.1.0_x64-setup.exe`（211 MB，内置 WebView2 离线安装器）+ `.sig` updater 签名 |
| Windows MSI `pnpm tauri build --bundles msi` | 最终包 | ✅ `JFileRescue_0.1.0_x64_{en-US,zh-CN}.msi` + `.sig`。首次构建因 `M&J Group` 中的 `&` 导致 candle CNDL0104 失败；改用 `src-tauri/wix/main.wxs` 自定义模板（厂商名预转义为 `M&amp;J Group`，注册表路径单反斜杠）后通过，渲染后的 wxs 已核对 |
| 生产二进制启动冒烟 `target/release/jfilerescue.exe` | 真实 WebView2 | ✅ 窗口标题 `JFileRescue`，WebView2 进程启动，8 s 后仍存活，正常结束 |
| updater 签名 | 最终包 | ✅ 密钥对在 `~/.tauri/jfilerescue.key`（仓库外，**请备份**），公钥已写入 `tauri.conf.json`，`.sig` 已生成 |
| 真实安装 / 真实磁盘业务验收 | 安装冒烟 / 真实业务 | ⏳ **未执行**（需管理员权限与真实删除场景；见待办） |
| **CI（GitHub Actions，仓库 luckySpro/jfilerescue）** | 目标平台 | macOS 与 Windows runner 上 `cargo test` / `clippy -D warnings` / 前端构建全部通过（macOS 侧曾暴露 3 处仅 unix 编译的问题：`FileTypeExt` 导入、`signal` 函数指针转整数 lint、Apple 签名环境变量为空时 bundler 仍尝试导入证书；均已修复并由 CI 复验）。Windows runner 产出 NSIS + 2 个 MSI + `.sig`；macOS arm64 runner 产出 `.dmg` + `.app.tar.gz` + `.sig`（未签名/未公证，首次启动需右键打开）。Intel 构建首次使用已退役的 `macos-13` runner 永久排队，改为 `macos-15-intel` 后重跑，结果见发布记录 |
| macOS 构建、DMG、特权助手、FileVault/TCC | 目标平台 | 构建：✅ CI 上 arm64 已出包；**真机启动、FileVault/TCC、助手**：⏳ 未执行（无 Mac，无 Developer ID） |

## 发布记录 v0.1.0（2026-10-21）

- 仓库 `luckySpro/jfilerescue`，Release https://github.com/luckySpro/jfilerescue/releases/tag/v0.1.0，标签 `v0.1.0` → `56debca`，工作流运行 `39519560489`：test×2、build×3（windows-latest / macos-latest arm64 / macos-15-intel）、publish 全部成功。
- 13 个附件：Windows NSIS `x64-setup.exe` + `.sig`、MSI `zh-CN`/`en-US` + `.sig`、macOS `aarch64.dmg` / `x64.dmg`、`aarch64.app.tar.gz` / `x64.app.tar.gz` + `.sig`、`latest.json`。
- 公开后独立复验：4 个平台下载 URL HEAD 200 且大小与附件一致；`latest.json` 经官方 `/releases/latest/download/` 与 GH-Proxy 镜像均可取到，4 个 platform 的 signature 字段为完整 minisign 签名；用 `scripts/verify-sig.mjs` 以 `tauri.conf.json` 内嵌公钥**密码学验证**已发布的 `x64-setup.exe` 与 `aarch64.app.tar.gz` 签名均有效（Ed25519，BLAKE2b 预哈希，keyid 匹配）。
- 发布过程中修复并记录：① 首次 `latest.json` 的 signature 为 `"Not Found"`（草稿附件无公开 URL，workflow 用 browser_download_url 读 `.sig` 失败）→ 手工按公开 `.sig` 重新生成并替换该附件，workflow 已改为通过 API 读取并校验前缀；② `macos-13` runner 已退役导致 Intel 任务永久排队 → 改 `macos-15-intel`；③ Apple 证书 secret 为空时 bundler 仍尝试导入 → 仅在设置时导出 Apple 环境变量。
- 未配置 Apple Developer ID / Windows Authenticode：macOS 包未签名未公证（首次启动需右键打开），Windows 包无发行者签名（SmartScreen 提示）。updater 私钥仅在 `~/.tauri/jfilerescue.key` 与仓库 secret，**请备份**。
- 升级链路验证：目录与签名已验证；**真实客户端"旧版 → 新版"自动更新**需等 0.1.1 发布后用已安装的 0.1.0 验证。

## 待办（需要用户环境配合）

1. **真实 NTFS/exFAT 镜像测试**：以管理员运行 `node scripts/make-test-images.mjs`，然后 `cargo test -p recovery-core --test images`。
2. **真实磁盘验收**：管理员启动 `JFileRescue.exe`（或 `sudo jfr`），对一个 U 盘删除文件后执行指定目录找回，核对 SHA256。
3. **macOS**：在 Mac 上 `pnpm tauri build --bundles app,dmg`，`node scripts/make-test-images.mjs` 生成 APFS 镜像跑测试；签名助手需 Developer ID（`helper/macos/README.md`）。
4. **发布**：GitHub secrets `TAURI_SIGNING_PRIVATE_KEY`（`~/.tauri/jfilerescue.key` 内容）与密码；Apple/Windows 代码签名证书按需。

## 已知限制

- NTFS 无删除时间戳，界面"删除时间"取 MFT 记录修改时间作近似。
- FAT 删除后 FAT 链被清零，非连续文件只能按连续假设恢复（与业界工具一致），可恢复性标为"中"。
- APFS：扫描依赖残留的 B-tree 叶节点；FileVault 卷在设备层为密文，不可用。
- 全盘雕刻无原文件名；指定目录模式下不做雕刻（结果无法归属目录）。
- SSD + TRIM 会显著降低成功率，界面已提示。
