# JFileRescue 验证记录

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
| macOS 构建、DMG、特权助手、FileVault/TCC | 目标平台 | ⏳ **未执行**（本机无 macOS）。源码按 cfg 分离并通过 Windows 侧编译检查；macOS 路径未经编译 |

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
