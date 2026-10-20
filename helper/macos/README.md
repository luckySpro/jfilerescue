# macOS 特权助手

GUI 以普通用户运行；原始设备 `/dev/rdiskN` 需要 root。助手只做一件事：验证调用方代码签名后，以 `O_RDONLY` 打开指定的原始磁盘设备并通过 XPC 传回文件描述符。

## 状态

- `helper.m` / `com.mjgroup.jfilerescue.helper.plist`：源码与 LaunchDaemon 清单，已写好安全检查（Team ID 代码签名要求、路径正则 + 设备类型校验、只读 + CLOEXEC）。
- **尚未接入 `tauri build`**：需要 Apple Developer ID 证书才能让 SMAppService 注册助手（未签名的助手会被系统拒绝）。在有证书的 macOS 构建机上：
  1. `clang -fobjc-arc -framework Foundation -framework Security helper.m -o jfilerescue-helper`
  2. 放入 `JFileRescue.app/Contents/Library/LaunchDaemons/` 与 `Contents/MacOS/`，用同一 Team ID 签名。
  3. 将 `REPLACE_TEAM_ID` 替换为真实 Team ID。
  4. Rust 侧 `recovery-platform::macos` 通过 XPC 请求 `open_ro` 并用 `device_from_fd` 包装（接口已预留）。
- 无证书时的替代路径：使用 `sudo jfr …` 命令行，或 GUI 内提示用户以 `sudo` 启动。

## 不做的事

不挂载、不写入、不修复、不接受任意路径、不常驻 GUI 进程之外的逻辑。
