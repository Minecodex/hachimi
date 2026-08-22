# macOS 移植开发计划

更新时间：2026-08-22

目标：在保持 Windows 支持不变的前提下，让 Hachimi Desktop 支持 macOS（**仅 arm64 / Apple Silicon**；x86_64 / Intel Mac 明确放弃，决策与理由见 P5 第 6 节）。
本文是移植计划的总索引与基线；每一步的详细任务见同目录的分阶段文档。

> 进度（2026-08-22）：**P1 已完成实现与自动化验证**——macOS arm64 上 `pnpm build:debug` 全量构建通过、`cargo test --workspace` 86 个测试套件全绿、`clippy --all-targets -D warnings` 与 `cargo fmt --check` 干净、语音 CPU 推理（SenseVoice/VITS）实测通过。沙箱 / CEF 内嵌浏览器 / Computer Use 按既定降级路径运行。剩余唯一人工项：GUI 冒烟（`pnpm dev` 开窗验收）。

## 现状基线（2026-08-22 源码盘点）

- 全仓库 `cfg(windows)` / `target_os = "windows"` 共 **229 处、46 个文件**；`cfg(unix)/macos` 分支仅 10 处，基本从零起步。
- 7 个 crate 存在 `[target.'cfg(windows)'.dependencies]`：`hachimi-voice`、`hachimi-skills`、`hachimi-process`、`hachimi-workspace`、`hachimi-computer`、`hachimi-sandbox`、`apps/desktop/src-tauri`。
- **核心 Agent 逻辑基本平台中立**：`hachimi-agent` / `hachimi-control-plane` / `hachimi-channel-providers` / `hachimi-audit` / `hachimi-approvals` / `hachimi-storage` 合计需改动的平台分支不足 50 行（ADR-0001 边界带来的收益）。
- 多数公共 API 在非 Windows 下已能编译（stub / fail-closed 设计），但没有任何一个系统级功能在非 Windows 有真实实现。
- `apps/desktop/src-tauri/binaries/` 已存在 `*-aarch64-apple-darwin` sidecar 产物：arm64 侧车编译链路部分就绪；x86_64 已决策放弃（见 P5 第 6 节）。

## 移植面总表

| #   | 移植面                                                                | 关键位置                                                                                                    | 工作量            | 阶段    |
| --- | --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ----------------- | ------- |
| 1   | 构建链去 powershell.exe 硬编码                                        | `tauri.conf.json:7-8`、`package.json:17,30,34,37-38,58`、14 个 `.ps1`                                       | 中                | P1      |
| 2   | build.rs 原生资产校验平台化                                           | `apps/desktop/src-tauri/build.rs:143-298,413-484`                                                           | 中                | P1      |
| 3   | Tauri bundle macOS 段（dmg/app、icns、权限描述、entitlements）        | `tauri.conf.json:42,44,66-74`                                                                               | 中                | P1      |
| 4   | unix PTY 后端（替代 ConPTY）                                          | `crates/hachimi-process/src/pty.rs:263-272`、`pty/conpty.rs`（855 行）                                      | 中                | P1      |
| 5   | sherpa-onnx mac 运行时（第一期 CPU-only）                             | `crates/hachimi-voice`、`scripts/run-with-rust.mjs:77-111`、`resources/native/`                             | 中                | P1      |
| 6   | 小项批处理（shell/URL 打开、浏览器检测、存储路径、前端默认 shell 等） | 见 P1 文档第 5 节                                                                                           | 小                | P1      |
| 7   | Seatbelt 沙箱后端（替代 AppContainer）                                | `crates/hachimi-sandbox/`（全部 Windows 实现）                                                              | **大**            | P2      |
| 8   | macOS Computer Broker（ScreenCaptureKit + CGEvent）                   | `crates/hachimi-computer/src/platform/windows.rs`（1722 行）                                                | **大**            | P3      |
| 9   | CEF mac 嵌入（NSView 嵌入模型，含原型验证）                           | `crates/hachimi-cef-host`、`embedded_browser.rs:1001-1060`                                                  | **大 / 风险最高** | P4      |
| 10  | 签名 / 公证基建（codesign + notarize）                                | 新增                                                                                                        | 中                | P5      |
| 11  | CI mac 矩阵 + 发布证据链对称扩展                                      | `.github/workflows/ci.yml`、`windows-release-gate.yml`、`scripts/release/`                                  | 中-大             | P5      |
| 12  | E2E 方案选型（WKWebView 无 WebDriver）                                | `scripts/desktop-e2e/`                                                                                      | **大 / 存疑**     | P5      |
| 13  | ~~x86_64 mac 资产矩阵~~（已放弃，arm64-only，见 P5 第 6 节）          | —                                                                                                           | 不做              | —       |
| 14  | hachimi-skills watcher mac 后端                                       | `crates/hachimi-skills/src/watcher.rs`（非 Windows 现为 `NativeWatchUnsupported`，P1 dev 冒烟确认显式降级） | 小-中             | P2 附属 |

## 阶段划分

| 阶段 | 文档                                                               | 目标                                                                                | 出口标准                                                |
| ---- | ------------------------------------------------------------------ | ----------------------------------------------------------------------------------- | ------------------------------------------------------- |
| P1   | [phase-1-build-bootstrap.md](phase-1-build-bootstrap.md)           | 解开构建链，arm64 出可运行的 dev 包；沙箱 / 内嵌浏览器 / Computer 显式降级          | macOS arm64 上 `pnpm dev` 跑通，Workbench 核心流程可用  |
| P2   | [phase-2-sandbox-seatbelt.md](phase-2-sandbox-seatbelt.md)         | hachimi-sandbox 新增 Seatbelt 后端，解除 stdio MCP / workspace 写执行的 fail-closed | mac 上沙箱 readiness 可 attested，workspace 写/执行恢复 |
| P3   | [phase-3-computer-use.md](phase-3-computer-use.md)                 | hachimi-computer 新增 `platform/macos.rs` Broker                                    | Computer Observe/Act 在 mac 上可用（权限授权后）        |
| P4   | [phase-4-cef-embedded-browser.md](phase-4-cef-embedded-browser.md) | CEF mac 打包 + NSView 嵌入原型验证；不过则长期走扩展通道降级                        | 内嵌浏览器可用，或降级路径定型并文档化                  |
| P5   | [phase-5-release-ci-e2e.md](phase-5-release-ci-e2e.md)             | 签名公证、dmg 产物、CI 矩阵、E2E 方案                                               | mac 通道进入正式发布证据链                              |

## 关键风险

1. **CEF 嵌入模型（P4，最高风险）**：当前是 Win32 HWND 父子窗口 + 与 WebView2 兄弟窗口 z-order 叠加（`crates/hachimi-cef-host/src/tab_manager.rs:1464-1535`，mac 下为空桩）。mac 上 WKWebView 用 CALayer 合成，CEF 子 NSView 的叠加/z-order 行为无直接对应，**必须先做原型验证**。降级选项：mac 只用浏览器扩展通道（loopback TCP，本身跨平台），现有 `RuntimeMissing` 降级路径已支持。
2. **E2E 驱动栈（P5，唯一没有现成答案的项）**：tauri-driver 依赖 WebDriver，WKWebView 无 WebDriver 支持；现 E2E（wdio + msedgedriver + UI Automation 断言 toast + 注册表清理）在 mac 上整体不可用。备选：safaridriver（能力弱）、自研 AX 驱动、关键路径降级为 Rust 集成测试。
3. **沙箱 fail-closed 传导（P2）**：workspace 沙箱执行与 stdio MCP fail-closed 在 `SandboxBackend` 上（`hachimi-capabilities/src/mcp_supervisor.rs:411-450`），P2 不落地则这些能力在 mac 上整体不可用。P1 期间显式降级并在 UI 呈现 degraded 状态。
4. **签名/公证（P5）**：Gatekeeper 对带 quarantine 属性的 CEF helper app 严格，CEF `.app` + 多 helper + dylib 的 codesign/notarize 流程是独立基建项，不要拖到最后。

## 移植原则

1. **不动 Windows 行为**：所有改动以新增 `cfg(target_os = "macos")` / 平台参数化分支为主，Windows 路径保持不变；Windows 侧 CI 必须全程绿。**已批准的例外**：git 来源全平台统一改为系统 git（拆除 Windows pinned MinGit 体系，见 P1 第 3 节决策）。
2. **沿用现有抽象边界**：`SandboxBackend`（`hachimi-sandbox/src/lib.rs:81-89`）、`ComputerBroker`（`hachimi-computer/src/lib.rs:132-185`）、PTY 分叉点（`hachimi-process/src/pty.rs:263`）均已就位，新平台实现落在边界内，不引入新抽象层。
3. **fail-closed 语义保持**：mac 上未实现的能力必须走显式降级/报错路径（项目已有此惯例），不允许静默回退到无保护执行。
4. **仅 arm64**：macOS 只支持 Apple Silicon；x86_64 / Intel Mac 不出正式产物、不做 CI 验证（决策与理由见 P5 第 6 节）。
5. **脚本风格统一**：`.ps1` 逻辑迁移为跨平台 `.mjs`（仓库已有 `run-with-rust.mjs` / `prepare-workspace-worker.mjs` 风格），不引入 pwsh 运行时依赖。
6. 参考实现（codex 的 macOS Seatbelt 沙箱等）按 AGENTS.md 规则固定版本、登记来源后再对标。
