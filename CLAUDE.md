# CLAUDE.md

JFileRescue — Tauri 2 + Rust workspace + Vue 3. 只读文件找回工具（Windows / macOS）。

## 命令

```sh
pnpm install
cargo test --workspace                 # Rust（含 FAT32 内存镜像集成测试；test-images/*.img 存在时跑真实镜像）
pnpm typecheck && pnpm build           # 前端
pnpm tauri dev / pnpm tauri build
pnpm icons                             # public/icon.svg → src-tauri/icons/*
pnpm screenshots                       # Playwright → docs/screenshots/
node scripts/make-test-images.mjs      # Windows 需管理员 (diskpart)；macOS hdiutil
```

## 约定

- 源设备永远只读；`recover::same_disk` + `recovery_platform::same_disk` 在 Rust 端拒绝同盘目标，前端只是提示。
- 领域逻辑全部在 `crates/recovery-core`，不引入 GUI 依赖；平台差异只在 `crates/recovery-platform`。
- 用户可维护的配置（签名库）通过界面管理，`signatures.toml` 只是存储格式；内置项只能停用不能删除。
- 不捆绑 GPL 组件（PhotoRec/TSK 等）；签名规则为公开事实，解析实现自有。
- 新增 Tauri command 要同时在 `src-tauri/src/lib.rs` 注册、`src/api.ts` 加类型、`src/demo.ts` 加演示分支。
- 四语：`src/i18n.ts`，zh-CN 为基准，缺 key 回落 zh-CN。
- 署名 M&J Group · 473276@qq.com；Bundle ID `com.mjgroup.jfilerescue` 不可随意改。
