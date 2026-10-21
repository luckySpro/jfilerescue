## JFileRescue 0.1.0

跨平台**只读**文件找回工具 · Windows / macOS · M&J Group

### 下载

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows 10/11 x64 | `JFileRescue_0.1.0_x64-setup.exe` | 推荐。NSIS 安装器，内置 WebView2 离线安装器，可完全离线安装 |
| Windows 10/11 x64 | `JFileRescue_0.1.0_x64_zh-CN.msi` / `_en-US.msi` | MSI（企业部署） |
| macOS 12+ Apple Silicon | `JFileRescue_0.1.0_aarch64.dmg` | **未签名/未公证**：首次启动请右键 → 打开，或 `xattr -dr com.apple.quarantine /Applications/JFileRescue.app` |
| macOS 12+ Intel | `JFileRescue_0.1.0_x64.dmg` | 同上 |

`*.sig` 与 `latest.json` 供应用内自动更新使用，无需手动下载。

### 功能

- **指定目录找回**：按原目录路径找回被删除的文件，保留文件名与目录结构（NTFS MFT / FAT·exFAT 目录项 / APFS 文件系统树残留节点）
- **全盘找回**：元数据 + 52 种文件签名逐扇区雕刻未分配空间，可找回格式化、分区表损坏后的文件
- 源磁盘永远只读；恢复目标与源同盘时在 Rust 端拒绝
- 可恢复性评分（数据簇覆盖率 + 结构校验）、预览、CSV 导出、会话持久化
- 签名库在界面内增删改 / 启停 / 导入导出 / 恢复默认
- 简体中文 / 繁體中文 / English / 日本語，浅色 / 深色
- 命令行 `jfr`（随源码构建）用于脚本与无人值守

### 权限

- Windows：以管理员运行（UAC 提示）以只读打开物理磁盘与卷
- macOS：系统盘需「完全磁盘访问」；FileVault 卷在设备层为密文，不可雕刻

### 已验证 / 未验证

详见仓库 `docs/VALIDATION.md`。本版本 Windows 安装包经本机真实启动冒烟；FAT32 真实卷端到端（扫描 → 恢复 → SHA256 一致）通过；NTFS / exFAT / APFS 真实卷与 macOS 真机启动**尚未验证**，欢迎反馈。
