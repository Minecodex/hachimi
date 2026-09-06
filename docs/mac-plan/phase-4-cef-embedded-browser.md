# P4：CEF 内嵌浏览器 macOS 版

- 状态：阶段 A/B/C 实现已完成（2026-08-29）；桌面合成 runtime 的真机 GUI 目视验收待用户，性能项（IOSurface 零拷贝帧通道）后移 P5
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

### 阶段 A：原型验证（1-2 周，决定是否继续）—— ✅ 已完成（2026-08-29）

- [x] 最小原型：Tauri mac 窗口 + cef-rs helper bundle。关键结论：**跨进程 NSView 直接嵌入在 mac 上不可行**（CEF render 进程的 NSView 无法挂进 browser 进程窗口，Win32 HWND 父子模型在 mac 无对应），改走 **OSR + 桌面侧合成**：CEF 以 windowless 模式渲染，帧回传桌面进程，由原生 NSView overlay 盖在 WKWebView 上方（等价 Windows 兄弟 HWND z-order 模型）。
  - [x] 叠加/z-order 正确（前端 UI 浮层能盖住浏览器区域）：实测 overlay 为 content view 最顶 subview（WKWebView 之上），快照读回 CEF 蓝色帧像素。
  - [x] 坐标映射与 Retina scale 正确（点击命中）：400×300pt overlay 快照为 800×600px（scale=2），点击经 IPC 命中页面 DOM 计数。
  - [x] 键盘/滚轮事件路由：鼠标链路实测通过（真实 NSEvent → overlay → IPC `Input.dispatchMouseEvent` → 页面计数）；键盘/滚轮走同一 IPC 通道（host 已有 `send_key_event`/`send_mouse_event`），桌面侧接入留 P4-C。
  - [x] 窗口 resize / move 跟随：OSR 模型下是 overlay frame 更新 + `WasResized`——P4-C 已实现（host `set_bounds` 接 `was_resized`，桌面 `cef_overlay.sync_tab` 同步 overlay frame；冒烟实测 set_bounds 后按新尺寸出帧）。
- [x] 结论写入本文档"原型结论"一节：继续 / 转 OSR / 转降级 → **继续，路线即 OSR**。

#### 原型结论（2026-08-29）

原型产物：`crates/hachimi-cef-host`（mac 启动序列 + OSR 渲染处理器 + bundle）与 `apps/desktop/src-tauri/examples/cef_osr_probe.rs`（桌面合成探针，自校验自终止）。实测输出 `COMPOSITE-PROBE-OK`（`PROBE z_order=true input=true frames_seen=3`）。

已打通的宿主侧链路：

- mac 启动序列（`main.rs`）：autoreleasepool → `LibraryLoader`（按可执行路径是否含 `.app/Contents/Frameworks/` 判定 helper）→ `cef::api_hash` 校验（缺了会在 `cef_command_line_create` SIGTRAP）→ command line → `run_cef_host`。
- `host_app.rs`：mac 必须 `multi_threaded_message_loop=0` + 主线程 `cef::run_message_loop()`；`--hachimi-osr` 开关 + `windowless_rendering_enabled`；OSR 下允许 `parent_hwnd=0`。
- `tab_manager.rs`：OSR 下 `WindowInfo::set_as_windowless(null_mut())`；`HachimiRenderHandler.on_paint` 把 BGRA 帧写到 `<profile>/frames/<tab>.bgra`（原型期帧通道，P4-C 换 IOSurface/共享内存）；OSR 下 `create_browser` 初始 URL 不生效（停在 about:blank），已在 `browser_created` 显式 `frame.load_url(url)` 修复。
- `bundle-hachimi-cef-host.rs` mac 分支产出 `target/cef-bundle/hachimi-cef-host.app`（主 app + 5 helper + framework）；cef-dll-sys 的 mac 构建依赖 **ninja**（固定在 `target/tools/ninja`，构建时挂入 PATH）。

踩坑记录（P4-B/C 勿再踩）：

1. tao `run_return` + `ControlFlow::Poll` 永不返回（poll 模式事件不枯竭）——必须在 `MainEventsCleared` 时置 `Exit`，泵一轮即回。
2. 开发机系统代理（127.0.0.1:7890）会劫持 CEF 内 loopback 请求——探针/测试一律 `--no-proxy-server` 并清 proxy 环境变量。
3. CEF `on_paint` 输出 BGRA，`NSBitmapImageRep` 32bpp 是 RGBA，直接塞红蓝互换；且须让 rep 自持像素数据（null planes + copy），否则 layer 内容悬垂。
4. WKWebView 内容是跨进程 CALayer，`cacheDisplayInRect` 进程内快照拍不到它（读回全 0）；z-order 用 subviews 顺序 + overlay 自身快照双信号证明。
5. CEF sandbox feature 需要 main.rs 在 helper 进程里 `cef_sandbox_initialize`（bundle 自带 `libcef_sandbox.dylib`），否则 helper 崩 exit_code=5 —— P4-C 已接入，构建恢复默认 feature。

P4-C 追加踩坑记录：

6. mac 上 `cef::quit_message_loop()` 无法终止 `cef::run_message_loop()`（无 NSApplication 时消息泵不响应 quit）——host 改为主线程 `do_message_loop_work()` 手动泵 + 后台命令线程退出标志。
7. 测试夹具的 HTTP 服务必须先读完请求头再写响应：socket 带未读数据直接 close 会触发内核 RST，Chromium 报 `ERR_CONNECTION_RESET`（时序敏感，表现为随机失败）。
8. 帧内容断言要等"标记色帧"而不是"第 N 帧"：OSR 首几帧是空白页，加载完成前像素是白的。

### 阶段 B：打包链路（原型通过后）—— ✅ 已完成（2026-08-29）

- [x] `scripts/prepare-desktop-runtime.mjs`（`pnpm cef:prepare` 入口，即原计划 `build-cef-host.mjs` 骨架的落点）mac 分支完整实现：pinned CMake 3.31.8（macos-universal，SHA256 `d1449f96…`）+ Ninja 1.13.1（ninja-mac，SHA256 `da779779…`）下载校验、`cargo build --bin hachimi-cef-host`、macosarm64 归档 SHA256 pin（`e3d268c8…`）校验、`.app` bundle 产出。注意 mac 打包含**真实 bin**（Windows 打 cdylib 只需 `--lib`）。
- [x] `bundle-hachimi-cef-host.rs` manifest 平台参数化（windows-x64 / macos-arm64 常量分 cfg）+ helper `.app` 产出结构定型（主 app + 5 helper + framework，247 文件全量哈希清单）。
- [x] `build.rs verify_cef_runtime()` mac 校验段：`target_os` 运行时参数化（跨编译安全），mac 断言 `macos-arm64` 平台、归档 SHA256、`.app/Contents/MacOS` 宿主与 framework 二进制标记；缺失时提示 `corepack pnpm cef:prepare`。
- [x] `embedded_browser.rs` mac 实现：`resolve_host_executable` 解析 `.app/Contents/MacOS/hachimi-cef-host`（resource dir / 可执行相邻 / target/cef-bundle 三级回退）；`validate_runtime` 校验 Info.plist、framework 二进制、基础 helper、`runtime-manifest.json`；OSR 路线下 `native_window_handle` 返回 0（windowless），spawn 追加 `--hachimi-osr`，`attach_window` 为空操作（无父窗口可设）。
- [ ] 签名/公证接入：**移交 P5**（签名基建在 P5 落地；本阶段已预留稳定 bundle id `com.hachimi.cef-host`，helper app 的 codesign/notarize 是 P5 首批接入对象）。

### 阶段 C：功能补全（按 OSR + 桌面合成路线，2026-08-29 修订）—— ✅ 实现完成（2026-08-29），真机目视验收待用户

- [x] **host OSR 语义补全**：`set_bounds` 接 `was_resized()` + `notify_screen_info_changed()`、`set_visible`/`activate_tab` 接 `was_hidden()`；`HachimiRenderHandler` 实现 `screen_info`（`device_scale_factor` 来自 `CefBounds.scale_factor`）+ `view_rect` 输出逻辑像素（物理/scale）。冒烟实测：1600×1200@2x → 帧 1600×1200，`set_bounds` 后按新尺寸重渲。
- [x] **输入注入 IPC**：`CefHostCommand::SendInput` + `CefInputEvent`（mouse_move/mouse_button/mouse_wheel/key），host 侧映射 `send_mouse_move/click/wheel_event`/`send_key_event`（真实输入复用 `user_input` 语义：清 agent 导航策略 + input_epoch）；mac 键码→Chromium VK 映射表与 NSEvent 修饰位换算在 `hachimi-browser/src/mac_input.rs`（含单测）。冒烟实测：注入点击命中页面 DOM 计数。
- [x] **桌面合成 runtime**：`apps/desktop/src-tauri/src/cef_overlay.rs`——按 tab 管理 `HachimiCefOverlayView`（NSImageView 子类，挂 workbench content view 最顶层）；host 写完帧发 `FrameReady` IPC 事件驱动桌面读帧合成（无轮询）；物理 px ÷ scale 回点 + AppKit 左下原点翻转在桌面侧完成；NSEvent（鼠标/滚轮/键盘）→ `SendInput`；`SetVisible`/隐藏联动。objc2 系从 dev-dependencies 提为 `[target.'cfg(target_os = "macos")'.dependencies]`。**真机 GUI 目视验收（渲染清晰度/滚动手感/焦点切换）待用户**。
- [x] 快捷键 mac Cmd 语义：`shortcut_for_key` mac 分支 Cmd+L/T/W/R + Cmd+←/→（Windows Ctrl/Alt 不变），测试按 cfg 分流。
- [x] mac 等价冒烟：`scripts/test-cef-host.mjs`（`pnpm test:cef:mac`）——ready/建 tab/Retina 帧/输入注入/resize 重渲/导航错误/干净退出全断言，进程组清理不泄漏 helper。
- [x] `is_internal_sidecar` 覆盖 mac helper 进程名（`starts_with("hachimi-cef-host")`）；`managed_sandbox_runtime.rs` 经核已无需改（P2 重写后不再读 runtime-manifest）。
- [x] **CEF mac 沙箱接入**：`main.rs` 在 helper 进程里 `cef::sandbox::Sandbox::new()` + `initialize`（先于 framework 加载；bundle 自带 `Libraries/libcef_sandbox.dylib`）；带默认 `sandbox` feature 构建冒烟全绿，`prepare-desktop-runtime.mjs` 已摘掉 `--no-default-features`。CEF 进程沙箱是 Chromium 内建 Seatbelt，与 P2 workspace Seatbelt 独立不冲突。
- [x] **shutdown 修复（计划外）**：mac 上 `quit_message_loop` 无法终止 `run_message_loop`（无 NSApplication），改为主线程 `do_message_loop_work` 手动泵 + 退出标志，冒烟验证 exit 0。
- [ ] （性能后段，可后移 P5）帧通道升级：`on_accelerated_paint` + IOSurface 零拷贝（cef `accelerated_osr` feature + wgpu），替换文件落盘通道。

## 4. 降级方案定型（~~若原型不通过~~ → 不适用：原型已通过，走 OSR + 桌面合成路线）

- [x] ~~mac 上 `embedded_browser` 保持 `RuntimeMissing` 降级~~（原型通过，未走降级）。
- [x] ~~浏览器扩展通道作为 mac 浏览器能力主路径~~（未采用；扩展通道保持现有独立能力定位）。
- [x] ~~决策与理由写入 ADR-0004 增补~~（路线决策与依据已记录于本文档"原型结论"一节）。

## 出口标准

1. 原型结论明确并记录在案。
2. （通过路径）mac 上 CEF 内嵌浏览器可用：标签管理、导航、下载、权限与 Windows 对齐；签名公证后的 `.app` 在干净机器 Gatekeeper 通过。
3. （降级路径）降级方案文档化，mac 浏览器扩展通道验证可用。
4. Windows CEF 链路与 CI 无回归。
