# P2：macOS Seatbelt 沙箱后端

- 状态：未开始
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
| `runtime_attestation.rs:22,118`                     | 策略版本 `hachimi-windows-appcontainer-v3` + canary 证明                                                                             | 运行时证明                |
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

- [ ] **参考来源登记**：codex 的 macOS seatbelt 实现，按 AGENTS.md 规则固定 commit 并登记 `docs/references/` 后再对标。
- [ ] 新增 `crates/hachimi-sandbox/src/seatbelt.rs`：SBPL profile 生成（从持久化 grant 集生成 subpath 规则）、`sandbox-exec` 包装启动、进程组杀树。
- [ ] `SandboxBackend` 的 mac 实现挂接；`SANDBOX_POLICY_VERSION` 新增 `hachimi-macos-seatbelt-v1`，与 `runtime_attestation.rs:22` 的 Windows 版本并存。
- [ ] `setup.rs` mac 分支：每用户 runtime 落盘（路径 P1 已平台化）、系统 git 解析（见 P1 第 3 节，不再有 managed-git 布局校验）、安装 marker；**无 ACL 步骤**。
- [ ] `runtime_attestation.rs` mac 分支：canary 四件套（文件写拒绝、未授权读拒绝、断网、进程树回收）用 seatbelt 语义重写。
- [ ] `process_backend.rs` 的 `ALLOWED_ENVIRONMENT` 平台化（`HOME`/`PATH`/`TMPDIR`/`SSH_AUTH_SOCK` 等 mac 集合）。
- [ ] `path_security.rs` mac 分支：`dev/ino` 文件身份、symlink 拒绝、系统目录保护；删除 NTFS 专属检查的 mac 编译路径。
- [ ] `runtime_manager.rs` mac 分支：setup helper 调用去 CREATE_NO_WINDOW（mac 天然无窗口）。
- [ ] 新增 `tests/macos_smoke.rs`：对标 `windows_smoke.rs` 的威胁模型（symlink 逃逸、句柄/fd 继承探针、进程组杀树、断网、`.git` 只读）。
- [ ] `hachimi-sandbox-canary` 二进制的 `--assert-job` / `--write-handle` 增加 mac 语义。
- [ ] 挂接点收口：`gateway_process.rs:50-62`（P1 已抽象）、`hachimi-workspace` 沙箱执行、`hachimi-capabilities` stdio MCP supervisor——逐项从 fail-closed 切换为 mac seatbelt 后端。
- [ ] `scripts/setup-portable-sandbox.ps1` 的 mac 等价（mjs），断言 marker 与断网边界。
- [ ] sidecar 哈希登记：`build.rs:413-440` 的 `{name}.exe` 参数化，mac sidecar 纳入校验。

## 5. 语义差异备忘（设计决策点，实现前需确认）

1. **进程组杀树 vs Job Object**：mac 进程组可被 `setsid` 逃逸，无法达到 Job Object 的强制层级。选项：接受降级（记录为已知差异）或用 `libproc` 枚举后裔兜底。需写进文档。
2. **SBPL 全局生效**：`sandbox-exec` 的 profile 对进程内所有线程生效，粒度比 AppContainer token 粗。确认现有 grant 模型（按 Run/Session）能否映射。
3. **macOS 15+ 对 sandbox-exec 的态度**：私有 API 长期弃用警告，需评估可接受性（codex 仍使用）与长期替代（Endpoint Security 不在本期范围）。

## 出口标准

1. mac 上 `SandboxCapabilityReport` 可达 `Enforced`（canary 全过），UI readiness 与 Windows 对齐。
2. workspace 写/执行、stdio MCP 在沙箱内可用；未授权路径/网络 fail-closed。
3. `tests/macos_smoke.rs` 通过；Windows `windows_smoke.rs` 与 CI 全绿无回归。
4. ADR-0001 增补或新增 ADR 记录 Seatbelt 后端的语义差异决策。
