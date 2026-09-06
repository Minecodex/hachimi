# P5：发布链路

- 状态：进行中——开发版通道（方案 A：GitHub Release 直发 ad-hoc 签名 dmg）已于 2026-09-05 落地并本机验收（`pnpm build:installer` 出 dmg、重签断言、quarantine 模拟、启动冒烟）；正式发布项（付费签名公证、release gate、E2E）未动
- 前置：P1 完成；签名公证建议在 P4 阶段 B 之前具备（CEF helper app 需要）
- 目标：mac 通道进入正式发布流程：签名/公证、dmg 产物、CI 矩阵、E2E 方案、发布证据链对称扩展（仅 arm64）。

## 1. 签名与公证基建

- [ ] 申请/配置 Apple Developer ID（Application + Installer 证书）与 notarize 凭据（App Store Connect API key 或 notarytool profile）。CI secrets 建议：`APPLE_CERTIFICATE`（p12 base64）/ `APPLE_CERTIFICATE_PASSWORD` / `APPLE_SIGNING_IDENTITY` / `APPLE_API_KEY_PATH`（或 `APPLE_ID`+`APPLE_PASSWORD`+`APPLE_TEAM_ID`）/ `KEYCHAIN_PASSWORD`——与 tauri-action 约定对齐。
- [ ] codesign 顺序：由内向外——dylib（sherpa-onnx 三件套）→ sidecar 二进制 → CEF framework 与 helper `.app` → 主 `.app`；全部带 hardened runtime + 对应 entitlements。**注意**：CEF 嵌套产物要在 `tauri build` 之前签好（`cef:prepare` 产出 `target/cef-bundle/` 后立刻签，tauri 只签主 app 外层，不会替你签嵌套的 helper）。
- [ ] entitlements 设计：主 app 需 `com.apple.security.cs.allow-unsigned-executable-memory`（CEF/JIT 常见需求，实测确认）；CEF renderer helper 需 `com.apple.security.cs.allow-jit`；禁用 App Sandbox（主 app 不启用——CEF 内容进程已由 Chromium Seatbelt 沙箱接管，见 P4 阶段 C；workspace 侧由 P2 Seatbelt 后端接管）。
- [ ] `notarytool submit` + staple 接入发布脚本；Gatekeeper 冒烟（干净机器 quarantine 下载验证）。
- [ ] Tauri 侧：`tauri.conf.json` `bundle.macOS` 补 `signingIdentity` / `provider` / `entitlements` 配置。`Info.plist` 目前只有 `NSMicrophoneUsageDescription`（语音够用；若内嵌浏览器页面要用摄像头再补 `NSCameraUsageDescription`）。

## 2. 安装包与 portable 产物

> 通道决策（2026-08-30）：**开发版走方案 A——GitHub Release 直发 ad-hoc 签名 dmg**（免费，无 Developer ID / 无公证），用户首次打开右键 → 打开 放行一次；正式发布再升级付费签名 + 公证（第 1 节保留）。分发说明必须写在 release notes。Homebrew cask 为可选增强通道（brew 下载无 quarantine，更顺滑），暂不做。

- [x] bundle targets 增加 `"dmg"`（`tauri.conf.json`）；icon 已含 `.icns`（P1）。
- [x] `scripts/release/build-installers.mjs` 增加 mac 分支（`buildMacInstaller`：`tauri build --bundles app,dmg`，清理 `bundle/macos` + `bundle/dmg`；Windows 行为不变）。
- [x] 出包验收：`pnpm build:installer` 在 mac 产出 `Hachimi_<version>_aarch64.dmg`；链路内含自内向外 ad-hoc 重签（修复 tauri 拷贝资源后留下的失效资源封条）+ `codesign --verify --deep --strict` fail-closed 断言 + hdiutil dmg（带 Applications 快捷方式）。本机实测：dmg 挂载安装、quarantine 模拟下 `spctl` 按预期拒绝（对应用户右键打开）、移除 quarantine 后启动存活。
- [x] 打包期修复（实测暴露）：`build.rs` 为 mac 追加 `-Wl,-rpath,@executable_path/../Resources/sherpa-onnx`——此前 `.app` 离开 dev shell 就因 `@rpath/libsherpa-onnx-c-api.dylib` 找不到而秒退（dev 一直靠 `DYLD_FALLBACK_LIBRARY_PATH` 掩盖）。
- [x] ~~`scripts/build-portable.ps1` 的 mac 等价~~（方案 A 下 portable 非必需，优先级降低；P2 移交项保持 P5 内）。
- [ ] 自更新：Tauri updater 的 mac 通道（`.app.tar.gz` + 签名）。注意：**当前各平台均未接 updater 插件**（`tauri.conf.json` plugins 为空），这是全新能力而非与 Windows 并存——首发可不做，用户手动下载新版 dmg 即可。

> 备注：打包机器需预留磁盘余量（`.app` ≈1.4GB，dmg 临时目录会再复制一份；本机磁盘打满曾导致拷残缺包出现离奇启动 panic，先 `df -h` 确认）。

## 3. CI 矩阵

现状：`.github/workflows/ci.yml:12-108` 5 个 job 全部 `runs-on: windows-latest`；`windows-release-gate.yml` 用 `[self-hosted, windows, x64]` runner。

- [x] `ci.yml` 增加 `macos-14`（arm64）job：rust 静态检查 + `cargo test` + CEF host 冒烟（2026-08-30 落地的最小回归网；前端测试仍由 Windows job 承担，未含 release gate）。
- [ ] 新增 `mac-release-gate.yml`（或扩展 windows-release-gate 为多平台）：签名公证后的 dmg / portable 产物、产物 manifest、证据上传。
- [ ] `publish-release.yml:127-138` 的 `gh release create` 资产 glob 增加 `*.dmg` / `*.app.tar.gz`。

## 4. 发布证据链对称扩展

现状（fail-closed 设计，需对称扩展而非绕过）：

- `scripts/release/verify-evidence.mjs:15` 硬编码要求 `windows_standard_user,windows_elevated` 两类 gate。
- `scripts/release/evidence.mjs:390,401,505,519-520` 工件扩展名 `.exe/.msi/.zip` 硬编码。
- `scripts/release/version-check.mjs:112-117` 校验 `build-portable.ps1` / `test-package-licenses.ps1` 内容一致性。

任务：

- [ ] gate 名与工件扩展名参数化（按平台维度建模：`{platform}_{kind}`），mac 对应 gate 定义——注意 **mac 无"标准用户 vs elevated"的等价门禁**，证据模型需要调整（设计上对齐而非硬凑）。
- [ ] mac 安装/升级/卸载的发布门禁脚本（对标 `test-windows-release.ps1` / `test-windows-standard-user.ps1`）：干净用户安装、数据迁移、Gatekeeper 验证。
- [ ] `external-staging-gate.yml`（self-hosted Windows）评估 mac 等价 staging 环境的可行性或显式豁免。

## 5. E2E 方案（风险项，需选型决策）

现状：`scripts/desktop-e2e/` 基于 wdio + tauri-driver + msedgedriver（WebView2 栈）；`run.mjs:60-64` 硬编码 `tauri-driver.exe`/`msedgedriver.exe`；`support/processes.mjs:40-114` 操作注册表与 `taskkill.exe`；`support/windows-toast.mjs` 用 UI Automation 断言 toast。

**核心问题：WKWebView 无 WebDriver 支持，tauri-driver 在 mac 不可用。**

候选方案（实现前做选型 POC，结论写 ADR）：

1. **自研测试钩子**：在 app 内编译期 feature 注入测试 IPC（查询 UI 状态、注入输入），用 Rust 集成测试 + 少量 AX 驱动覆盖关键路径。可控性最高，工作量中。
2. **safaridriver 驱动 WKWebView**：能力很弱（不支持任意 WKWebView，仅 Safari），大概率不可行，先排除性验证。
3. **AX（Accessibility）驱动**：AppleScript / AXUIElement 驱动整个 app，无需 WebView 内部钩子，但断言语义粗、稳定性差。
4. **降级策略**：mac 的 E2E 只覆盖非 WebView 层（Rust 集成测试 + gateway/沙箱/CEF host 的进程级测试），WebView 层依赖与 Windows 共享的前端单测 + 手工验收清单。

任务：

- [ ] 选型 POC 与 ADR。
- [ ] `run.mjs` / `processes.mjs` / `windows-toast.mjs` 按平台分支或重写（toast 断言在 mac 走 UNUserNotificationCenter，需签名后才稳定弹出）。
- [ ] `scripts/prepare-desktop-e2e-tools.ps1:24-35` 的 mac 对应物（按选型定）。
- [ ] `scripts/desktop-stress/` 的 `chrome.exe`/`taskkill` 分支补 darwin 路径（`pkill` / 进程组）。

## 6. x86_64 / Intel Mac 支持（已决策：放弃）

- 决策（2026-08-22）：**macOS 正式产物仅 arm64（Apple Silicon）**——不出 x86_64 dmg，不建 x86_64 CI 与证据链。
- 理由：
  1. Apple 已官宣 macOS 26 Tahoe 是最后一个支持 Intel Mac 的系统版本，macOS 27（2026 秋）起 Intel 机型无法升级；Rosetta 2 到 macOS 28（2027 秋）基本移除——Intel Mac 只剩存量、没有增量。
  2. GitHub 已于 2025-12-04 退役 `macos-13` Intel runner，免费池仅剩过渡性的 `macos-15-intel`，开源生态正在批量停止发布 Intel 产物。
  3. 本项目原生资产按架构分包（CEF、sherpa-onnx、sidecar），双架构意味着资产/签名/证据链成本翻倍；universal2/lipo 因 CEF 分架构发包而不可行。
- 保留项：不主动破坏源码级可移植性（`cfg` 分支不按 mac 架构区分）；有需要的用户可自行以 `x86_64-apple-darwin` target 构建，但官方不提供签名/公证/支持。
- 可逆性：若未来确有需求，发布链路届时已平台参数化，加一条 Intel 腿是增量工作而非架构改动。

## 7. 杂项收尾

- [ ] `scripts/reset-portable-data.ps1`/`.cmd` 的 sh/mjs 等价（含 mac 凭据清理：keychain 条目，替代 `cmdkey.exe`）。（方案 A 下 portable 缓议，此项随之后移）
- [ ] `scripts/assert-windows-toast.ps1` 的 mac 等价或按平台跳过（随 E2E 选型一起定）。
- [x] `scripts/release/check-disk-space.ps1` 跨平台化（2026-09-05：新增 `check-disk-space.mjs`，`statfsSync` 实现，接入 `build-installers.mjs` mac 分支最低 8GiB 前置门槛；Windows gate 保留 ps1）；`test-package-licenses.ps1` 跨平台化仍待做。
- [x] `docs/ROADMAP.md` 与 ADR 增补 mac 语义（2026-09-05：ROADMAP 的 Browser/Computer、跨平台验证行与"当前范围"更新；ADR-0004 追加 macOS Browser host 修订段；ADR-0001 的 Seatbelt 修订段 P2 已写；ADR-0005 平台中立无需改）。
- [x] `AGENTS.md` 如涉及平台约定则同步更新（复核：AGENTS.md 无平台特定约定，无需改）。

## 出口标准

1. mac arm64 签名公证 dmg 进入正式发布，证据链与 Windows 对称。
2. CI mac 矩阵常态绿；发布门禁 mac 通道 fail-closed。
3. E2E 方案定型并落地（或降级策略文档化）。
4. ROADMAP 与 ADR 更新完成。
