# P3：macOS Computer Use Broker

- 状态：未开始
- 前置：P1 完成；与 P2 无依赖关系，可并行开发
- 目标：`hachimi-computer` 新增 `platform/macos.rs`，实现 `ComputerBroker` trait；Computer Observe/Act 在 mac 上可用。
- 备注：这是单一最大移植面（`platform/windows.rs` 1722 行），但 trait 边界干净，可作为独立模块落地，不影响其他阶段。

## 1. 现状盘点

- `crates/hachimi-computer/src/lib.rs:132-185` — `ComputerBroker` trait 已就位；`platform.rs:255-336` 每个方法均有 `cfg(not(windows))` 的 unsupported 实现；`computer_runtime_health` 非 Windows 返回 `computer_unsupported_os`（:31-40）。
- `crates/hachimi-computer/Cargo.toml:22-40` — `windows-capture` + `windows` crate（Graphics_Capture 等）仅 cfg(windows)。
- `src/lib.rs:753-759` — `LaunchApp` 校验要求 app_id 以 `.exe` 结尾（共享代码里的 Windows 假设）。
- `examples/stress_fixture.rs` — Win32 `CreateWindowExW` GUI 测试夹具，非 Windows 仅打印提示。

### Windows 实现清单（`platform/windows.rs`，均需 mac 对应物）

| 能力            | Windows 实现                                                           | 位置                |
| --------------- | ---------------------------------------------------------------------- | ------------------- |
| 屏幕捕获        | windows-capture crate / Windows.Graphics.Capture                       | :22-31, :263-344    |
| 窗口枚举/身份   | EnumWindows、GetWindowThreadProcessId、QueryFullProcessImageNameW      | :952-1032           |
| 版本资源        | GetFileVersionInfoW / VerQueryValueW                                   | :952-1032           |
| 应用身份        | AppUserModelID（SHGetPropertyStoreForWindow）、打包应用元数据          | :833, :849-892      |
| 验签            | PowerShell `Get-AuthenticodeSignature`                                 | :909-944            |
| 图标            | SHGetFileInfoW + GDI DIB→PNG                                           | :739-821            |
| 提权/受保护判定 | TokenIntegrityLevel 与 desktop 名                                      | :1073-1156          |
| 输入注入        | SendInput / SetCursorPos（鼠标/键盘/Unicode 文本）                     | :1182-1320          |
| 窗口操作        | ShowWindow / MoveWindow / PostMessageW(WM_CLOSE) / SetForegroundWindow | —                   |
| 应用启动        | LaunchApp 动作                                                         | :478                |
| 用户输入标记    | GetLastInputInfo                                                       | platform.rs:229-238 |

## 2. macOS 方案映射

| Windows 机制                                             | macOS 替代                                                                          | crate / 权限                                                                |
| -------------------------------------------------------- | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Windows.Graphics.Capture                                 | **ScreenCaptureKit**（SCContentFilter 按窗口/屏幕）                                 | `screencapturekit` crate 或 objc2 桥接；需 **Screen Recording** 权限（TCC） |
| EnumWindows / 窗口枚举                                   | `CGWindowListCopyWindowInfo`                                                        | core-graphics                                                               |
| SendInput / SetCursorPos                                 | **CGEvent**（CGEventPost、CGWarpMouseCursorPosition、Unicode 键盘事件）             | core-graphics；需 **Accessibility** 权限（TCC）                             |
| GetLastInputInfo                                         | `CGEventSourceSecondsSinceLastEventType`                                            | core-graphics                                                               |
| AppUserModelID                                           | Bundle Identifier（`NSRunningApplication` / `NSWorkspace`）                         | objc2-app-kit                                                               |
| Get-AuthenticodeSignature（PowerShell）                  | **SecCode 静态验签**（SecCodeCopyGuestWithAttributes / SecStaticCodeCheckValidity） | security-framework                                                          |
| SHGetFileInfoW 图标                                      | `NSWorkspace.icon(forFile:)` / `icon(forFileType:)` → PNG                           | objc2-app-kit                                                               |
| GetFileVersionInfoW                                      | Info.plist（CFBundleShortVersionString）                                            | core-foundation / plist                                                     |
| TokenIntegrityLevel / desktop 名（提权与受保护目标判定） | 无直接对应；用 SIP/rootless 保护进程检测 + 权限态重新设计                           | 决策点，见第 3 节                                                           |
| ShowWindow / MoveWindow / WM_CLOSE / SetForegroundWindow | AX API（AXUIElement）或 `NSRunningApplication.activate`                             | 窗口移动/关闭需 Accessibility 权限                                          |
| LaunchApp `.exe` 校验                                    | `open -a` / `NSWorkspace.openApplication`；校验改 bundle id / `.app`                | lib.rs:753-759 需平台化                                                     |

## 3. 设计决策点（实现前确认）

1. **提权/受保护目标判定**：Windows 用完整性级别 + desktop 名拒绝操作受保护目标；macOS 无等价概念。选项：检测目标进程是否 root 拥有 / 是否受 SIP 保护，或仅依赖 TCC 权限态。结论需写入 ADR-0004 增补。
2. **权限引导 UX**：Screen Recording 与 Accessibility 均需用户到系统设置授权，首次授权需重启应用（Accessibility）或不需（Screen Recording 可动态）。需要在产品层做权限状态检测与引导（Tauri 侧 + `tauri.conf.json` 的 `bundle.macOS` 描述字段）。
3. **Retina 坐标**：CGEvent 用全局点坐标，截图为像素，需统一 scale factor 换算（BackingScaleFactor）。

## 4. 任务清单

- [ ] 新增 `crates/hachimi-computer/src/platform/macos.rs`，按 `ComputerBroker` 逐个方法实现；`platform.rs` 的 `cfg(not(windows))` unsupported 桩替换为 macos 分支。
- [ ] `Cargo.toml` 增加 `[target.'cfg(target_os = "macos")'.dependencies]`：core-graphics / core-foundation / objc2 / objc2-app-kit / security-framework / screencapturekit（按选型定）。
- [ ] `lib.rs:753-759` `LaunchApp` 的 `.exe` 校验平台化（mac 校验 `.app` / bundle id）。
- [ ] `computer_runtime_health` mac 分支：报告 TCC 权限状态（Screen Recording / Accessibility 是否已授权），驱动 UI 引导。
- [ ] `examples/stress_fixture.rs` 新增 AppKit 版 GUI 夹具（用于 capture/input 压测）。
- [ ] `scripts/desktop-stress/run.mjs:55,216` 的 WGC 压测 fixture 增加 mac 分支或按平台跳过。
- [ ] 单元/集成测试：窗口枚举、截图、输入注入（可用 AX 自检回路）；权限缺失时 fail-closed 的报错路径。

## 出口标准

1. mac 上 Computer Observe（窗口枚举 + 截图）与 Act（鼠标/键盘注入、窗口操作、启动应用）可用。
2. 权限未授权时返回结构化降级错误，UI 引导到系统设置。
3. `cargo test -p hachimi-computer` 在 mac 与 Windows 全绿。
