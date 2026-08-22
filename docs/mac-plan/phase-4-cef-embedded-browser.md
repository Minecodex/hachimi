# P4：CEF 内嵌浏览器 macOS 版

- 状态：未开始
- 前置：P1 完成（构建链平台化）；建议与 P2/P3 并行，但**先做原型验证再投入完整实现**
- 目标：CEF 嵌入浏览器在 mac 上可用；若原型验证不通过，则定型"mac 长期走浏览器扩展通道"的降级方案并文档化。
- 风险：**全移植计划最高风险项**。窗口嵌入模型在 mac 无直接对应，需原型先行。

## 1. 现状盘点

### 已就绪（mac 适配量小的部分）

- `crates/hachimi-cef-host/src/bin/bundle-hachimi-cef-host.rs:36-39` — 已有 `cfg(target_os = "macos")` 分支调用 `cef::build_util::mac::bundle(...)`；`Cargo.toml:21-22` 的 `[package.metadata.cef.bundle] helper_name` 正是 mac helper `.app` 打包所需。
- `cef = 151.2.0` crate 跨平台；cef-builds.spotifycdn.com 提供 `macosarm64` minimal 归档（`macosx64` 不需要，x86_64 已放弃）。
- IPC 层（`ipc.rs`，stdin/stdout JSON Lines）、崩溃重启 supervision（`embedded_browser.rs:884-958`）、进程启动管道（`hachimi-process-policy` 的 `tokio_command`）全部跨平台可复用。
- `windows_entry.rs`（cef_sandbox 静态库引导入口）在 mac **不需要**——mac 沙箱由 helper app bundle 结构完成，`main.rs:6-13` 普通路径直接可用。

### 需要改造的部分

| 位置                                                                    | 现状                                                                                                | 改造                                                                                                             |
| ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `bundle-hachimi-cef-host.rs:53-64`                                      | `write_runtime_manifest` 硬编码 windows-x64 归档名 + SHA256                                         | 按 target 参数化为 `macosarm64`，补齐归档哈希                                                                    |
| `apps/desktop/src-tauri/build.rs:235-298`                               | `verify_cef_runtime()` 断言 `platform == "windows-x64"`、`.exe`/`libcef.dll`                        | mac 分支校验 `.app` + `Chromium Embedded Framework.framework` 布局                                               |
| `embedded_browser.rs:1001-1041`                                         | `resolve_host_executable` / `validate_runtime` 写死平面 DLL 布局                                    | mac：`.app/Contents/MacOS/...` + Frameworks + helper apps，路径解析重写                                          |
| `embedded_browser.rs:1043-1060`                                         | `native_window_handle` 非 Windows 直接报错                                                          | mac：`window.ns_window()` 取 NSWindow，再取 contentView 的 NSView\*                                              |
| `tab_manager.rs:1470-1535`                                              | `valid_parent_window` / `reparent_window` / `move_window` / `show_window` 非 Windows 是**空操作桩** | AppKit 实现：`addSubview` / `setFrame` / `setHidden`                                                             |
| `tab_manager.rs:238-244`                                                | `sys::HWND` 强转 + `WindowInfo::set_as_child`                                                       | mac 的 `cef_window_handle_t` 是 NSView\*，set_as_child 语义可用                                                  |
| `tab_manager.rs:1201-1231`                                              | 快捷键检测基于 Windows virtual-key code                                                             | cef 的 KeyEvent 在 mac 也填 `windows_key_code`（chromium 统一 VK），大概率可复用；需验证 Cmd/Ctrl 语义与键位映射 |
| `host_app.rs:13-40`                                                     | `--hachimi-parent-hwnd=` 参数                                                                       | 参数语义平台化（mac 传 NSView 指针值）                                                                           |
| `main.rs:1-18`                                                          | `windows_subsystem` + 沙箱引导门                                                                    | cfg(windows) 限定，mac 无需改                                                                                    |
| `managed_sandbox_runtime.rs:503`、`permission_settings_commands.rs:167` | 读 `runtime-manifest.json`、白名单 `hachimi-cef-host` 进程名                                        | 适配 `.app` 内可执行名 / 签名身份                                                                                |

## 2. 核心风险：NSView 嵌入与 z-order

当前设计（`tab_manager.rs:1502-1521` 注释）：CEF 子窗口与 WebView2 是**兄弟窗口**，靠 Win32 z-order 叠加。mac 上：

- Tauri/Wry 的 WKWebView 用 CALayer 合成，CEF 子 NSView 与其叠加的层级/裁剪/事件命中行为与 Win32 完全不同。
- 坐标系差异：mac 左下原点、点单位，需处理 Retina scale 换算。
- 若 NSView 叠加验证不通过，备选方案：
  1. **离屏渲染（OSR）**：CEF 渲染到纹理，合成进前端——改动大、性能需评估。
  2. **降级为浏览器扩展通道**：mac 不提供内嵌浏览器，走现有 Chrome/Edge 扩展 + loopback TCP（本身跨平台），`RuntimeMissing` 降级路径已支持。

## 3. 任务清单（按执行顺序）

### 阶段 A：原型验证（1-2 周，决定是否继续）

- [ ] 最小原型：Tauri mac 窗口 + cef-rs helper bundle，把 CEF 浏览器作为 NSView 嵌入 WKWebView 窗口的指定区域，验证：
  - [ ] 叠加/z-order 正确（前端 UI 浮层能盖住浏览器区域）
  - [ ] 坐标映射与 Retina scale 正确（点击命中）
  - [ ] 窗口 resize / move 跟随
  - [ ] 键盘/滚轮事件路由
- [ ] 结论写入本文档"原型结论"一节：继续 / 转 OSR / 转降级。

### 阶段 B：打包链路（原型通过后）

- [ ] `scripts/build-cef-host.mjs`（P1 已建骨架）mac 分支完整实现：下载 `cef_binary_*_macosarm64_minimal.tar.bz2`（pin SHA256）+ mac 工具链构建。
- [ ] `bundle-hachimi-cef-host.rs` manifest 平台参数化 + helper `.app` 产出结构定型。
- [ ] `build.rs verify_cef_runtime()` mac 校验段。
- [ ] `embedded_browser.rs` 的 `resolve_host_executable` / `validate_runtime` / `native_window_handle` mac 实现。
- [ ] 签名/公证接入（与 P5 的签名基建对接；**CEF helper app 是 Gatekeeper 最严格的部分**，不要拖到最后）。

### 阶段 C：功能补全

- [ ] `tab_manager.rs` 四个窗口函数的 AppKit 实现（引入 objc2/objc2-app-kit 依赖，cfg(target_os = "macos")）。
- [ ] 快捷键处理的 mac 键位验证与 Cmd 语义适配。
- [ ] `scripts/test-cef-host.ps1`（P/Invoke 创建 Win32 窗口做 smoke）的 mac 等价冒烟。
- [ ] `managed_sandbox_runtime.rs` / `permission_settings_commands.rs` 的进程名/签名身份适配。
- [ ] CEF mac 沙箱语义确认（helper app sandbox 与 P2 Seatbelt 后端的关系：CEF 进程沙箱独立于 workspace 沙箱，注意两者不冲突）。

## 4. 降级方案定型（若原型不通过）

- [ ] mac 上 `embedded_browser` 保持 `RuntimeMissing` 降级，UI 文案与能力声明平台化（"内嵌浏览器当前仅 Windows 可用"）。
- [ ] 浏览器扩展通道（Chrome/Edge/Safari 扩展可行性评估）作为 mac 浏览器能力主路径。
- [ ] 决策与理由写入 ADR-0004 增补。

## 出口标准

1. 原型结论明确并记录在案。
2. （通过路径）mac 上 CEF 内嵌浏览器可用：标签管理、导航、下载、权限与 Windows 对齐；签名公证后的 `.app` 在干净机器 Gatekeeper 通过。
3. （降级路径）降级方案文档化，mac 浏览器扩展通道验证可用。
4. Windows CEF 链路与 CI 无回归。
