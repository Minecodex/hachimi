# P2：macOS Seatbelt 沙箱后端

- 状态：已完成（2026-08-23 实现并验证；实测见文末“出口标准（实测）”）
- 前置：P1 完成（构建链与降级路径已就位）
- 目标：`hachimi-sandbox` 新增 Seatbelt（sandbox-exec / SBPL）后端，实现 `SandboxBackend` trait；解除 workspace 写/执行与 stdio MCP 在 mac 上的 fail-closed 状态。
- 重要性：这是所有受限执行的前置——workspace worker 沙箱执行、process launcher、stdio MCP（`hachimi-capabilities/src/mcp_supervisor.rs:411-450` 生产路径强制走 SandboxBackend）全部 fail-closed 在此后端上。

## 1. 现状盘点（Windows 实现，全部需要 mac 对应物）

| 模块                                                | Windows 实现                                                                                                                         | 能力本质                  |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------- |
| `crates/hachimi-sandbox/src/appcontainer.rs:34-133` | `AppContainerSid`（DeriveAppContainerSidFromAppContainerName 等）                                                                    | AppContainer 身份         |
| `appcontainer.rs:136-205`                           | `icacls.exe` 给 AppContainer SID 打 NTFS ACL                                                                                         | 文件授权                  |
| `restricted_process.rs:20-156`                      | CreateProcessAsUserW + SECURITY_CAPABILITIES + PROC_THREAD_ATTRIBUTE_HANDLE_LIST + Job Object（KILL_ON_JOB_CLOSE）+ CreatePipe stdio | 受限进程启动              |
| `setup.rs:241-448`                                  | `icacls.exe` 操作 Restricted Code SID + AppContainer SID 双重 ACL；checkout/`.git` 读写/只读切换                                     | 安装与工作区授权          |
| `setup.rs:115-198`                                  | managed Git 校验（`cmd/git.exe` 布局）                                                                                               | 运行时校验（P1 已平台化） |
| `path_security.rs:69,94,174-199,384-475`            | ensure_ntfs / 所有权校验 / subst 别名 / volume serial + file index 身份 / reparse point / 8.3 短名 / 保留设备名                      | 路径安全                  |
| `runtime_attestation.rs:22,118`                     | Windows 策略版本 `hachimi-windows-appcontainer-v4` + canary 证明                                                                     | 运行时证明                |
| `runtime_manager.rs:276-322`                        | CREATE_NO_WINDOW 调 setup helper                                                                                                     | 每用户安装编排            |
| `process_backend.rs:20-41`                          | ALLOWED_ENVIRONMENT 全是 Windows 变量                                                                                                | 环境白名单                |
| `tests/windows_smoke.rs`                            | `#![cfg(windows)]`，cmd mklink /J、subst、net session 等                                                                             | 冒烟测试                  |

## 2. 已有可复用边界（不动）

- `SandboxBackend` trait（`lib.rs:81-89`）+ `SandboxCapabilityReport` 协议类型 —— mac 后端直接实现该 trait。
- `probe_windows_readiness` 非 Windows 返回 `Unavailable`（`lib.rs:190-197`），上游 fail-closed 行为跨平台。
- `process_backend.rs` 的 spawn / 有界输出 / 取消 / 超时逻辑 —— 只要求"launcher 可执行文件 + `--` 协议"，可复用（环境变量清单与路径大小写比较需平台化）。
- `validate_checkout_root` 的 unix 分支（canonicalize + symlink 拒绝）已存在；`path_file_identity` 用 `MetadataExt::dev/ino` 替代 `WindowsFileIdentity`。
- `git_alias.rs` 的 subst 别名机制在 mac 无需求（Seatbelt 用路径规则，无 ACL 继承问题），非 Windows 分支已返回 `Ok(None)`，天然关闭。

## 3. macOS 方案映射

| Windows 机制                           | macOS 替代                                             | 备注                                                                                 |
| -------------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------------ |
| AppContainer + 受限 token              | `sandbox-exec` + SBPL profile（每进程）                | 文件规则 `(allow file-read*/file-write* (subpath ...))`，`.git` 只读直接写进 profile |
| deny-all 网络（C2.1）                  | `(deny network*)`                                      | 语义等价                                                                             |
| NTFS ACL（icacls）                     | 不需要；授权边界由 SBPL 路径规则表达                   | setup.rs 的 ACL 切换逻辑在 mac 无需对应物                                            |
| Job Object 杀进程树                    | `setsid` + `killpg`（process group），或 `posix_spawn` | 需处理孙子进程逃逸（进程组 vs Job 语义差异要测试）                                   |
| HANDLE_LIST 句柄白名单                 | unix 默认 close-on-exec / 显式 fd 传递                 | 语义天然接近                                                                         |
| subst 驱动器别名                       | 无需求                                                 | —                                                                                    |
| ensure_ntfs / 8.3 短名 / 保留设备名    | 无对应威胁；保留 symlink / `/dev` / 系统目录拒绝       | `path_security.rs` 新增 mac 分支                                                     |
| CreateWellKnownSid(RestrictedCode)     | 无对应物                                               | attestation 语义需重新表达                                                           |
| canary（Job / 文件写 / 拒绝读 / 断网） | 同语义重写：进程组检测、profile 内写拒绝、断网验证     | canary 协议本身可移植                                                                |

## 4. 任务清单

- [x] **参考来源登记**：openai/codex `codex-rs/sandboxing` @ `4f39251a010a8bd7d692d25fb33832ff06f1635a`，快照在 `docs/references/openai/raw/OAI-PRODUCT-CODEX-SEATBELT-20260823-*`，normalized 说明在 normalized/ 同名文档；provenance 总账与 checker 的 fixedSources 已登记。
- [x] 新增 `crates/hachimi-sandbox/src/seatbelt.rs` + `seatbelt_base_policy.sbpl`：SBPL 生成（grants → read/write subpath 参数、`.git` 只读 regex + literal 双规则、写根锚点 unlink deny、`deny network*`、祖先 path-ancestors 元数据放行）、固定 `/usr/bin/sandbox-exec`。
- [x] `SandboxBackend` mac 实现：`MacOSSeatbeltReadinessProbe` + `PlatformSandboxProbe`/`platform_probe()` 分发；`current_policy_version()`（当前 mac 为 `hachimi-macos-seatbelt-v2`）+ `backend_dir_key()`（`sandbox/macos-seatbelt/`）平台化。
- [x] `setup.rs` mac 分支：`install_macos_marker()`（纯版本记录，无 ACL/无安装）；系统 git 复用 P1 解析器。
- [x] `runtime_attestation.rs` mac 分支：`attest_macos_runtime` 七件套 canary（deny-default 探活、fd 继承、授权写、授权外写/读拒绝、断网、进程组回收含孙进程）+ `attest_macos_workspace_boundaries` per-checkout 证明。
- [x] `process_backend.rs`：`spawn_with_seatbelt`（process_group(0) + killpg）、`ALLOWED_ENVIRONMENT` 平台拆分、`SandboxedChild` Drop 的 unix killpg。
- [x] `path_security.rs`：mac 走既有 unix 分支（canonicalize + symlink 拒绝）；P1 已把 `/var→/private/var` 的 canonicalize-first 修正进 storage 层校验。
- [x] `runtime_manager.rs`：probe 字段泛化为 `Arc<dyn SandboxBackend>`；mac repair = 写 marker + 重跑 attestation。
- [x] 新增 `tests/macos_smoke.rs`（10 项全过）：逃逸/断网/`.git` 只读/进程组回收/fail-closed 降级逐项实测。
- [x] canary 二进制 mac 探针：`--assert-seatbelt`（deny-default 探活，仅 PermissionDenied 算通过）+ `--read-fd3`（fd 继承探针）。
- [x] 挂接点收口：`main.rs`/`gateway_process.rs` → `platform_probe()`；PTY 终端 launcher 路径接 seatbelt（`seatbelt_terminal_args`）；stdio MCP 与插件 sidecar 经后端恢复；`permission_settings_commands` 名称过滤天然跨平台。
- [x] `.git` 租约语义落地：`SandboxLaunchSpec.git_metadata_writable` + `WorkspaceSandboxContext.git_metadata_writable` + `with_git_metadata_writable`，在 mutation 租约持有期逐启动放行（替代 Windows 的临时 ACL 升级）。
- [x] sidecar 哈希登记：`build.rs` 按平台参数化（mac 登记 canary + workspace-worker 真实哈希，其余占位）；sidecar 落盘补 unix 可执行位（0755）。
- [x] 附带项：skills watcher mac 轮询后备（`watcher.rs` 非 Windows 从 NativeWatchUnsupported 改为 250ms 快照 diff，root 集逐轮重取，含测试）。
- [x] ~~`scripts/setup-portable-sandbox.ps1` 的 mac 等价（mjs）~~——移交 P5 发布打包阶段（portable 包是发布产物）。
- [x] 后续回归修复（2026-09-05，真机首提报发现）：`setup.rs` 的 `protect_restricted_code_read_only` 漏了平台门控，在 mac 上 spawn 不存在的 `icacls.exe` 导致工作台"创建首提"/交互式变更报 `sandbox_acl_prepare_failed`；已补 `!cfg!(windows)` 空操作门（与同文件 grant/revoke/deny 一致），并新增 `prepare_workspace_acl_is_a_bookkeeping_noop_on_macos` 回归测试（macos_smoke 11/11）。

## 5. 语义差异备忘（设计决策点，实现前需确认）

1. **进程组杀树 vs Job Object**：mac 进程组可被 `setsid` 逃逸，无法达到 Job Object 的强制层级。已定：接受该残留差异，写入 ADR-0001 修订段；canary 实测进程组回收（含孙进程）。
2. **SBPL 全局生效**：profile 对进程内所有线程生效——与 hachimi 每进程启动模型天然匹配（每个受限操作一次性 Worker 进程）。
3. **macOS 15+ 对 sandbox-exec 的态度**：实测 macOS 26（Tahoe）arm64 可用；codex 同期仍依赖；Endpoint Security 迁移记为远期观察项。

## 出口标准（2026-08-23 实测）

1. ✅ mac 上 `SandboxCapabilityReport` 达 `Ready/Enforced`：`pnpm dev` 实跑日志确认 `backend=macos_seatbelt_v1 readiness=Ready os_enforced=true`（启动时七件套 canary 全过）。
2. ✅ workspace 受限写/执行、stdio MCP、PTY 终端（seatbelt 包装）在 mac 沙箱内实测可用；未授权路径/网络/`.git` 均 fail-closed（macos_smoke + worker_process + mcp_stdio 的 mac 变体全过）。
3. `tests/macos_smoke.rs` 10 项通过；Windows 侧核心 crate 交叉 check/clippy 通过，完整 Windows CI 待提交后验证。
4. ✅ ADR-0001 已追加 2026-08-23 Seatbelt 修订段（含进程组 vs Job 的残留差异声明）。
