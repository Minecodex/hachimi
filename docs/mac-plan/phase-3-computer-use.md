# P3：macOS Computer Use Broker

- 状态：进行中（M1/M2/M3/M4 已完成实现与自动化验证。真实截图与真实输入注入待维护者授予本机 TCC（屏幕录制 + 辅助功能）后，由 `examples/capture_probe` 与 macos 压测迭代复核）
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

1. **提权/受保护目标判定**（已定，M1 落地）：`elevated` = 目标窗口属主进程 uid==0（`proc_pidinfo`/`PROC_PIDTBSDINFO`）；`protected_desktop` 恒 false（mac 无安全桌面；SIP/TCC 边界由门禁与权限态覆盖）。待写入 ADR-0004 增补（M4）。
2. **权限引导 UX**：Screen Recording 与 Accessibility 均需用户到系统设置授权，首次授权需重启应用（Accessibility）或不需（Screen Recording 可动态）。需要在产品层做权限状态检测与引导（Tauri 侧 + `tauri.conf.json` 的 `bundle.macOS` 描述字段）。
3. **Retina 坐标**：CGEvent 用全局点坐标，截图为像素，需统一 scale factor 换算（BackingScaleFactor）。

## 4. 任务清单

- [x] 新增 `crates/hachimi-computer/src/platform/macos.rs`（M1）：`runtime_health` / `list_windows` / `foreground_window` / `read_identity` / `app_icon_png` / `user_input_marker` 已实现；capture/perform 为 M2/M3 显式桩。
- [x] `Cargo.toml` macOS 依赖（M1 选型落地）：objc2 / objc2-foundation / objc2-app-kit / core-graphics / core-foundation / libc（全部沿用锁内版本族 pin 进 workspace）；`objc2-screen-capture-kit` 0.3.2 留 M2。
- [x] `lib.rs:753-759` `LaunchApp` 的 `.exe` 校验平台化（M4 落地：mac 走 `open -a`，校验 bundle id/app 名/`Safari` 形；Windows 形状不变）。
- [x] `computer_runtime_health` mac 分支（M1）：TCC 屏幕录制 + 辅助功能探测 + macOS 14 门槛（SCScreenshotManager 前置）+ root 判定；新稳定错误码 `computer_capture_requires_macos14` / `computer_screen_recording_required` / `computer_accessibility_required`。
- [x] `examples/stress_fixture.rs` 新增 AppKit 版 GUI 夹具（M4：NSWindow + NSTextField，标题 `Hachimi Computer fixture`）。
- [x] `scripts/desktop-stress/run.mjs` mac 分支（M4：darwin real 模式跑 `captures_and_controls_the_macos_stress_fixture`；浏览器阶段在 CEF mac 落地（P4）前保持 Windows-only）。
- [x] M1 测试：句柄解析、identity 稳定性、真机枚举+前台窗口一致性回读（live test）。
- [x] M2 捕获（`platform/macos.rs`）：SCShareableContent 定位窗口 → SCContentFilter(desktopIndependentWindow) → SCScreenshotManager 单帧（原生分辨率，隐去光标）→ NSBitmapImageRep PNG → 复用 FrameStore；TCC 未授权/版本不足时 fail-closed 稳定错误码；`examples/capture_probe.rs` 端到端探针（枚举→前台→捕获→读帧→落盘）。
- [x] 捕获/输入测试（M2/M3）：
  - 纯单元测试：键名→ANSI 键码、修饰键去重/拒绝、鼠标按钮映射、窗口相对→全局坐标换算（19 项 mac 测试全绿）。
  - live 测试：真机枚举+前台一致性（M1）、TCC 未授权时捕获 fail-closed/授权后真实截图（M2）。
  - 新增 `captures_and_controls_the_macos_stress_fixture`（ignore，压测脚本驱动）：夹具窗口枚举→身份→截图→AX 移动/恢复迭代。

## 出口标准

1. mac 上 Computer Observe（窗口枚举 + 截图）与 Act（鼠标/键盘注入、窗口操作、启动应用）可用。
2. 权限未授权时返回结构化降级错误，UI 引导到系统设置。
3. `cargo test -p hachimi-computer` 在 mac 与 Windows 全绿。
