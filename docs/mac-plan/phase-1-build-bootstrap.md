# P1：构建链平台化与 arm64 dev 包

- 状态：已完成（2026-08-22 M0-M5 全部完成并实测，见"出口标准（实测）"；2026-08-30 复核确认唯一残留的受限环境变量清单项已被 P2 Seatbelt 后端覆盖，见第 6 节）
- 前置：无（移植第一步）
- 目标：macOS arm64 上 `pnpm dev` / `cargo build` 全链路跑通，产出可运行的 dev 包；沙箱、内嵌浏览器、Computer Use 显式降级（fail-closed / degraded 状态已在产品语义内）。

## 1. 构建命令咽喉（已完成）

现状：

- `apps/desktop/src-tauri/tauri.conf.json:7-8` — `beforeDevCommand` / `beforeBuildCommand` 内联 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/prepare-managed-git.ps1` 和 `build-cef-host.ps1`，mac 上直接失败。
- `package.json:17,30,34,37-38,58` — `build:portable` / `test:desktop:e2e:prepare` / `cef:prepare` / `test:windows:*` / `runtime:prepare` 直调 powershell.exe。

任务（落地情况）：

- [x] 新增 `scripts/prepare-desktop-runtime.mjs` 作为 beforeCommand 的统一入口：win32 转发 `build-cef-host.ps1`，darwin 创建 `target/cef-bundle/` 占位并打印 CEF 降级提示（CEF 编译链路 P4 补齐）。
- [x] 拆除 `scripts/prepare-managed-git.ps1` 与相关入口（全平台改用系统 git，见第 3 节已定决策）。
- [x] `prepare-speech-models.ps1` → `scripts/prepare-speech-models.mjs`（下载 + SHA256 + tar 解包，已在 mac 实测通过）；`ensure-speech-models.mjs` 直接调 mjs，ps1 已删除。
- [x] `package.json` 入口统一：Windows-only 脚本走新增的 `scripts/windows-only.mjs` 守卫（非 win32 打印跳过并退出 0）；新增 `check:mac` 子集；`cef:prepare` 指向 `prepare-desktop-runtime.mjs release`。

## 2. build.rs 原生资产校验平台化

现状：`apps/desktop/src-tauri/build.rs` 的三个校验函数硬编码 Windows：

- `verify_native_runtime()`（:143-180）— 校验 `resources/native/sherpa-onnx-1.13.4-directml/windows-x64/` 三 DLL 的 SHA-256。
- ~~`verify_managed_git()`~~ — 已删除（M0）。
- `verify_cef_runtime()` — 断言 `platform == "windows-x64"`、`hachimi-cef-host.exe` / `libcef.dll`。
- `register_sandbox_sidecar_hashes()` — 已先行修复：非 Windows 发占位空哈希保证 `env!` 可编译（mac 上 sidecar 校验失败即按 degraded 上报，P2 再接入真实后端）。

任务：

- [x] 校验函数按 `CARGO_CFG_TARGET_OS`/`CARGO_CFG_TARGET_ARCH` 参数化平台目录：`windows` → `sherpa-onnx-1.13.4-directml/windows-x64`，`macos+aarch64` → `sherpa-onnx-1.13.4/darwin-arm64`；CEF 校验保持 Windows-only。
- [x] 新增 `resources/native/sherpa-onnx-1.13.4/darwin-arm64/`（去掉 `-directml` 后缀）+ manifest.json（SHA-256 清单），build.rs 校验段复用同一函数。
- [x] `register_sandbox_sidecar_hashes()` 非 Windows 占位（P2 补齐 mac sidecar 登记）。

## 3. git 来源（已定：全平台跟随 codex，用系统 git）

- 决策（2026-08-22，2026-09-06 由 ADR-0006 完成架构收口）：**Windows 与 macOS 统一使用系统 git**；拆除 pinned MinGit + manifest attestation，并由统一 System Runtime 发现、探测和租约化注入。
- 参考来源（按 AGENTS.md 规则固定版本、登记）：openai/codex @ `4f39251a010a8bd7d692d25fb33832ff06f1635a`（main，2026-08-22 检索），`codex-rs/git-utils/`：
  - 所有 git 操作 `Command::new("git")` 直接走 PATH（`operations.rs:117`、`branch.rs:129`、`status.rs:30`、`info.rs:384`），全平台一致；
  - 内部基线 / ghost commit 用纯 Rust 的 `gix`（`baseline.rs`），完全不经 git 二进制；
  - 对不可信工作区的加固在配置与进程层而非二进制层：`-c core.hooksPath=<disabled>` 禁 hooks、`SAFE_BARE_REPOSITORY_CONFIG`、`scrub_non_inheritable_env_vars`、`GIT_OPTIONAL_LOCKS=0`、unix 进程组杀树 / Windows Job Object（`operations.rs:107-114`、`git_process.rs`）。
- **保留**：worker 显式绝对路径注入契约（`setup.rs:200-202`，"never resolve Git from their checkout current directory"）——与二进制来源无关，是防恶意仓库 git shim 的关键；系统 git 在 setup 期一次性解析为绝对路径后照旧注入。
- **拆除**（managed git 依赖链全清单，M0 已完成）：
  - [x] `trusted_git_runtime()`/`validate_managed_git()` 与旧 OnceLock 解析器已删除；`hachimi-system-runtime` 按宿主 Shell/Windows 环境 `PATH`、进程 `PATH`、平台候选依次探测，并按 `git_inspect`、`git_local_mutation`、`git_worktree` 能力接受 Git，不再设置版本下限。Finder 最小环境下会先发现 Homebrew Git，也允许通过探测的 Apple Git 2.39.5。
  - [x] `runtime_attestation.rs` `ManagedGitManifest` 哈希校验 → 已删除，sidecar 完整性校验保留。
  - [x] `build.rs` `verify_managed_git()` → 已整体删除。
  - [x] `scripts/prepare-managed-git.ps1`、`tauri.conf.json` resources 的 `managed-git/`、`build-portable.ps1` 的 `git.exe` 断言 → 已删除（installer 体积随之缩小）。
  - [x] `runtime_manager.rs` 的 `HACHIMI_MANAGED_GIT_EXECUTABLE` 环境变量传递 → 已删除。
  - [x] `setup.rs:458-494`（git common-dir 发现）、`git_alias.rs:41`、`hachimi-workspace/src/lib.rs:1204` 的消费点 → 已切换到新解析器，调用语义不变。
  - [x] 附带清理：`permission_settings_commands.rs` 的 bundled git 候选源、`runtime-health.tsx` 的 managed*git*\* 文案（替换为 `system_git_missing`）、`external-staging-gate.yml` 的 prepare-managed-git 步骤、`.gitignore` 条目。
  - [x] ADR-0001/0005 已追加 2026-08-22 修订段。
- **加固（跟随 codex）**：git.rs 主调用层本已有 `core.hooksPath=<NUL|/dev/null>` + `GIT_OPTIONAL_LOCKS=0` + env_clear；M0 补齐了 `diff.rs`/`browser.rs` 两个只读调用点的 hooksPath。
- **风险与对策**：① 用户未装 git → 应用与 Sandbox 正常启动，仅 Git 能力禁用并显示准确错误；安装后可点击重新检测；② 版本漂移 → 按能力探测并在 Worker 启动前复核文件身份；③ PATH 劫持 → 丢弃相对/项目路径，探测后只注入规范化绝对路径。
- **E2E**：桌面覆盖空仓库初始提交、索引/未跟踪文件不变、Git 缺失时禁用及恢复后刷新成功；release gate 另保留 macOS `.app` GUI 等价最小环境和 Windows standard-user 清理 PATH 的系统发现验证。
- 备选备查：GitHub Desktop 的 dugite-native 提供 pinned mac git 构建（全平台自带 git 的另一流派），因 codex 基线优先而不采用。

## 4. Tauri bundle macOS 段（已完成）

现状：`tauri.conf.json:42` targets 仅 `["nsis","msi"]`；`:44` icon 无 `.icns`（`icons/icon.icns` 已在仓库未引用）；`:66-68` resources 写死 windows-x64 DLL；`:70-74` 只有 `bundle.windows` 段。

任务（落地情况）：

- [x] bundle targets 增加 `"app"`（dmg 属 P5）。
- [x] icon 列表加入 `icons/icon.icns`。
- [x] 新增 `bundle.macOS` 段：`minimumSystemVersion: "13.0"`、`infoPlist: "Info.plist"`（新增 `Info.plist`，含 `NSMicrophoneUsageDescription`）。hardenedRuntime/entitlements 属 P5 签名基建，P1 不开。
- [x] resources 增加 `resources/native/sherpa-onnx-1.13.4/darwin-arm64/ → sherpa-onnx/` 映射；`target/cef-bundle/ → cef-runtime/` 跨平台保留。**注意**：`.app` 包内 dylib 的 `@rpath` 加载链路与签名属 P5（P1 dev 走 `--no-bundle`）。
- [x] `storage_layout.rs` Installed 分支加 mac：`~/Library/Application Support/com.hachimi.desktop`（WKWebView 数据 WKWebView 默认已在该处，无需 redirect）；`sandbox/windows/setup.json` marker 路径保持 Windows 专属（mac 在 P1 永不触碰，P2 Seatbelt 后端自带路径）。
- [x] 附带发现并开启：`tauri` crate 的 `macos-private-api` feature + `app.macOSPrivateApi: true`（透明 pet 窗口在 mac 的必需项）。

## 5. 小项批处理（已完成）

| 位置                                                            | 现状                                                           | 落地                                                                                                                                                                                                   |
| --------------------------------------------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `packages/workbench/src/terminal.tsx`                           | 默认 shell 写死 `powershell.exe`                               | `get_default_shell` 从统一 System Runtime 返回绝对路径、Shell 类型、交互/命令参数和 revision 的 `ShellLaunchSpec`；mac 使用账户登录 Shell，Windows 优先 PowerShell，缺失才降级系统命令解释器。         |
| `apps/desktop/src-tauri/src/browser_detection.rs`               | 注册表/where.exe 检测，非 Windows 空桩                         | mac：`/Applications` + `~/Applications` 的 `.app` 固定候选；`--version` 探测改为跨平台（creation_flags 仅 cfg(windows)）                                                                               |
| `browser_settings_commands.rs`、`browser_workspace_commands.rs` | `rundll32`/`explorer` 开 URL/文件夹，非 Windows 报 Unsupported | mac：`open <url>` / `open -R <path>`；linux：`xdg-open`；其余平台仍 Unsupported                                                                                                                        |
| `apps/desktop/src-tauri/src/main.rs` Skill 目录                 | `USERPROFILE`/`PROGRAMDATA`                                    | mac：`$HOME/.agents/skills` + `/Library/Application Support/Hachimi/skills`                                                                                                                            |
| `media_commands.rs`                                             | `#[cfg(any())] legacy_windows_speech` 死代码                   | 已删除（-197 行）                                                                                                                                                                                      |
| `crates/hachimi-agent/src/security_tools.rs`                    | 路径小写化                                                     | `cfg!(any(windows, macos))`（默认 NTFS/APFS 均大小写不敏感，注释说明）                                                                                                                                 |
| `crates/hachimi-policy/src/lib.rs` `normalize_case`             | 同上                                                           | 同上处理                                                                                                                                                                                               |
| `packages/workbench/src/timeline/message-markdown`              | 本地路径链接                                                   | POSIX 绝对路径**原本已支持**（`local-file-links.ts:19`），补了 POSIX 测试用例                                                                                                                          |
| `crates/hachimi-voice` 测试 cfg                                 | 4 个集成测试 `#[cfg(windows)]`                                 | 已放开（mac CPU 路径实测 30 过）；`native_paths_strip` 保持 Windows-only                                                                                                                               |
| `gateway_process.rs:50-62`                                      | 沙箱路径/probe 硬编码                                          | **未改**：probe 在非 Windows 天然返回 Unavailable，路径构造无副作用；后端选择挂接点属 P2                                                                                                               |
| mac clippy 死代码清理                                           | windows-only 代码在非 Windows 报 dead_code                     | cfg-gate：`path_security` 2 处、`git_alias` SubstDrive 系列、`computer/platform` FrameStore insert + 常量、`skills/watcher` 快照、`hachimi-motion` 引用修正、`hachimi-protocol` 测试 lint（HEAD 既有） |

## 6. unix PTY 后端（已完成）

- 现状：`crates/hachimi-process/src/pty.rs:263-272` 非 Windows 直接报错；`pty/conpty.rs`（855 行）是唯一实现。pipe 后端零改动。
- 落地：
  - [x] 新增 `crates/hachimi-process/src/pty/unix_pty.rs`：`portable-pty`（workspace pin `=0.9.0`）建 PTY，读写走 spawn_blocking + mpsc（镜像 ConPTY 控制契约），resize 走 `TIOCSWINSZ`，进程树终止用进程组 `killpg`（child 是 session leader，pgid=pid）；`launcher = Some(..)` 明确报错（沙箱启动器属 P2）。
  - [x] `src/tests.rs` fixture 换成 `sh`/跨平台脚本，4 个测试全部放开 cfg 并在 mac 实测通过（含 Ctrl-C 前台中断、幂等写、resize）。
  - [x] ~~`src/lib.rs:202-267` 的 `RestrictedProcessTemp` / `prepare_restricted_environment` 环境变量清单平台化~~（2026-08-30 复核：该函数只在 `restricted_launcher.is_some()` 的 Windows AppContainer 路径触发；mac 沙箱执行走 P2 Seatbelt 后端，`seatbelt.rs` 自行处理 TMPDIR/TEMP/TMP 白名单——此项对 mac 不适用，已被 P2 覆盖）。

## 7. sherpa-onnx mac 运行时（CPU-only，已完成）

- 落地：
  - [x] 引入官方 `sherpa-onnx-v1.13.4-osx-arm64-shared-lib`（已重新下载官方归档与本地副本比对 SHA-256 一致：`995d38d3…`），取 `libsherpa-onnx-c-api.dylib` + `libonnxruntime.{1.27.0,}.dylib` 落 `resources/native/sherpa-onnx-1.13.4/darwin-arm64/` + manifest.json。
  - [x] **重要发现**：上游包的 `libonnxruntime` dylib 自带签名损坏（`invalid signature`，arm64 上 dyld 直接 SIGKILL），已 adhoc 重签（`codesign --force --sign -`），manifest 记录的是重签后哈希并在 notes 字段说明；P5 接入 Developer ID 签名链。
  - [x] `run-with-rust.mjs` darwin 分支：`SHERPA_ONNX_LIB_DIR` + `DYLD_FALLBACK_LIBRARY_PATH`；另处理 workspace 级 cargo 命令在 mac 排除 `hachimi-cef-host`（Windows-only，插在 `--` 分隔符前）。
  - [x] `VoiceComputeMode::Auto` 在非 Windows 经既有 stub 回退 CPU；SenseVoice/VITS 推理测试 mac 实测通过。
  - [x] cpal/rodio 跨平台（CoreAudio），无需改。
- 备注：GPU 加速（CoreML EP）需自编译 onnxruntime + sherpa-onnx，**不在 P1 范围**，列为后续可选项。

## 8. 降级路径确认（P1 出口前必须逐项验证）

mac 上以下能力在 P1 保持显式降级，需确认 UI/诊断呈现正确而非静默失败：

- 沙箱：`probe_windows_readiness` 非 Windows 返回 `Unavailable`（`hachimi-sandbox/src/lib.rs:190-197`），workspace 写/执行与 stdio MCP fail-closed —— 确认 UI 展示 degraded。
- CEF 内嵌浏览器：`embedded_browser.rs:1043-1060` 非 Windows 报错措辞已平台化（指向 P4 文档），确认走 `RuntimeMissing` 降级（浏览器扩展通道不受影响）。
- Computer Use：`computer_runtime_health` 非 Windows 返回 `computer_unsupported_os`（`hachimi-computer/src/platform.rs:31-40`）。

## 已知环境问题（P1 实测发现，非移植缺陷）

1. **本机代理会截获 loopback 流量**：开发机配置了 `all_proxy` / macOS 系统代理时，reqwest 默认走系统代理。已修生产代码中所有 loopback 客户端（mcp_oauth 回调、`EnterpriseApiClient::with_loopback_endpoint`）强制 `no_proxy()`；跑测试仍建议 `env -u all_proxy`。
2. **上游 sherpa-onnx osx 包的 libonnxruntime dylib 签名损坏**（`invalid signature`，arm64 直接 SIGKILL）：已 adhoc 重签并记录（manifest notes）；P5 接入正式签名链。
3. **~~desktop 下载测试并发竞争~~（已修复并查明根因）**：`enterprise_outcome_indeterminate` 的确定性失败是三个叠加问题——测试 fixture 服务器的 3 秒创建期绝对期限在并行负载下先于请求到期、macOS/BSD 的 accept 继承 O_NONBLOCK 导致阻塞读变 WouldBlock、loopback 客户端走了系统代理。三者已分别修复（期限按请求刷新 + 连接归一化为阻塞 + 读超时 + 客户端 no_proxy），desktop 套件默认线程下 106/106 通过。
4. **watch 轮询后备只比较路径集合、漏报内容修改**（既有 bug，mac 首个行使者）：`watch.rs` 的 portable fallback 已改为按路径→(len, mtime) 快照比较，`worker_process` watch 测试在 mac 通过。
5. **HEAD 基线本身不是全绿的**（与本移植无关的既有问题）：`hachimi-protocol` 一个 clippy lint、若干前端 eslint/prettier 债务（avatar-motion-runtime / pet / ui / docs ui 样式目录）。已顺手修掉 protocol 的 lint 与 lib.rs 的 rustfmt 归一，其余留给 UI 线自行清理。
6. **扩展 hook/connector sidecar 的 bundle ACL 是 Windows 专属机制**：`grant/deny/revoke_restricted_code_access` 在非 Windows 改为文档化 no-op（Seatbelt 后端 P2 用 SBPL 路径规则表达同等授权；执行边界仍由 `SandboxBackend::spawn_restricted` fail-closed 把守），`hook_runtime` 测试在 mac 通过。

## 出口标准（2026-08-22 实测）

1. macOS arm64（M 系，本机）：`pnpm build:debug` 全量构建通过（`target/debug/hachimi-desktop` 产物生成）；`pnpm dev` 已实跑——应用正常启动、稳定运行、进程干净退出，pet 前端加载 VRM 成功，SenseVoice/VITS 均经 CPU 后端热身完成，沙箱按 `sandbox_repair_unsupported_os` 显式降级，系统 git 正常解析，前后端日志无错误。剩余纯目视项（桌宠渲染观感、Workbench UI、终端 zsh 交互、降级文案呈现）由维护者开窗确认。
2. Windows 侧无行为回归：改动以新增 cfg 分支/平台参数化为主；`hachimi-sandbox`/`hachimi-workspace`/`hachimi-process` 的 `x86_64-pc-windows-msvc` 交叉 `cargo check`/clippy 通过；完整 Windows CI 待提交后验证。
3. ✅ mac 上 `cargo test --workspace` 86 个测试套件全绿（含 desktop 106、workspace watch、voice CPU 推理、extensions hook、capabilities oauth）；`clippy --workspace --all-targets --all-features -D warnings` 与 `cargo fmt --all --check` 干净。
