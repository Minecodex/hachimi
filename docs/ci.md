# PR 与 CI 合并规则

日常开发使用功能分支和 PR。默认分支要求 GitHub Actions 的固定 `CI` 检查、同步最新主分支并解决审查讨论；失败、取消或意外跳过不能通过汇总门禁。禁止强推和删除，不设置管理员绕过。

## 执行层级

默认主分支为 `main`。正式支持 Windows x64 / macOS ARM64。

| 入口                      | 实际范围                                                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 普通 PR / main push       | Windows/macOS 原生源码、全部共享 UI/视觉；标准用户执行 Workbench 核心、审批恢复/重入和 Skill/MCP 设置三组桌面规格        |
| CI full / beta* / v* 候选 | 相同源码门禁和完整七组桌面规格，增加 Multi-Agent/Forge、Host/Channel、Office/Scheduled Tasks 和 Motion V5                |
| Windows Release Gate      | GitHub 托管 Windows 上构建一次 MSI/NSIS/便携 ZIP，校验同一 commit/version/hash 的不可变候选                              |
| 独立环境                  | standard-user/elevated 的真实安装升级、签名浏览器身份、OpenAI/Forge/企业组织/Channel 的真实外部 staging、正式 RC/GA 证据 |

桌面配置使用显式 `HACHIMI_DESKTOP_E2E_PROFILE=pr|release`，标准用户子进程通过参数接收同一范围，不依赖账户切换时环境变量是否继承。默认本地执行仍为 release 全套；诊断可显式选择 spec/grep，CI 保持固定完整范围。

PowerShell 夹具执行编码的就绪命令后检查实际输出标记，避免依赖启动横幅或把命令回显当作就绪。畸形 RPC 响应夹具先消费请求再关闭 stdin，仍由原生 hook 测试验证拒绝、禁用订阅、超时与取消行为。生产沙箱策略不变。

专用 `External Staging Gate` 和 `Publish Verified RC or GA` 已从 Actions 移除。Actions 不使用 self-hosted Runner、受保护 Environment、外部 API Secret 或人工配置的扩展 ID；真实环境测试继续保留独立脚本与六类证据校验，见 [独立验收规范](RELEASE_GATES.md)。

beta 和 alpha 发布仍验证同一候选的源码、制品及哈希；不附带真实安装身份或外部服务认证结论。完整环境未具备时，这些检查保持待验证，不作为 Actions 成功的一部分。没有定时任务。
