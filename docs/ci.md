# PR 与 CI 合并规则

本仓库默认主分支为 `main`。日常开发从最新主分支创建功能分支，将功能分支推送到组织仓库，再通过 PR 合并。

```sh
git fetch origin
git switch -c feature/your-change origin/main
git push -u origin feature/your-change
```

主分支由仓库规则集保护：必须通过 PR，必须通过 `CI` 检查，合并前必须同步最新主分支，禁止强制推送和删除。规则没有管理员绕过名单；目前不额外要求他人批准，PR 上的审查讨论必须解决。

`CI` 是固定名称的汇总检查。它在所有 PR 上运行，任一必需任务失败、取消或意外跳过都会失败，避免工作流名称或矩阵版本变化导致保护规则失效。检查来源限定为 GitHub Actions。

## 自动检查范围

保留 Windows 静态检查、Rust、UI/视觉、桌面 E2E、标准用户门禁和 macOS 原生测试。`CI` 汇总全部六个任务。

工作流也支持主分支 push 和手动运行。手动运行使用 Actions 页面的 Run workflow，选择待检查的分支；功能分支首次引入新工作流时，先创建 PR 触发检查。

Windows Release Gate 和 External Staging Gate 是独立发布验收，需要专用 runner、环境和发行候选，不能用 PR CI 成功替代。

CI 失败会阻止合并。修复失败后在同一个功能分支继续提交，重新运行检查；不要通过删除必需检查或设置管理员绕过来把失败当作通过。

桌面包的 Core Foundation / Core Graphics 测试依赖限定到 macOS。Cargo 测试启动器将绝对 PATH 中的系统 Git 解析为既有测试覆盖路径；生产 Worker 仍要求经过验证的 Git lease 与运行时版本，不增加生产 PATH 回退。

架构扫描使用 Git 的文件清单，遵守忽略规则并覆盖隐藏的 Storybook 配置，不依赖 runner 额外安装 ripgrep。UI 与桌面 E2E 失败时上传截图、差异、驱动日志和隔离测试目录中的应用日志，供定位环境或功能失败。

Windows Shell PTY 探测回复 ConPTY 的光标位置查询，并在读取线程仍工作时关闭控制台，随后等待输出结束。Git/工作区测试和桌面启动都经过同一个系统运行时入口；探测完成不能以绕过真实 PTY 能力校验来替代。

默认界面与代码字体随 UI 打包并锁定版本，Storybook 使用同一个样式入口，截图不依赖开发者机器预装字体。更新视觉基线前必须检查当前页面与差异，更新后重新执行正常比较；更新基线的运行本身不能代替视觉门禁。

视觉测试浏览器固定 `Asia/Shanghai` 时区，与任务计划 fixture 的时区一致，时间标签不依赖 runner 的系统时区。同一 PR 的新提交会取消旧提交仍在执行的 CI，门禁只接受当前提交的完整结果。

原生日期字段的 ISO 值单独断言，截图只隐藏该字段由 Windows 决定的日期文本格式，字段布局和其他页面内容仍完整比较。Windows Rust 任务同样检出 LFS 模型；桌面 E2E 按实际 WebView2 Runtime 版本选择 Microsoft 驱动，并将测试应用绑定到同一运行时目录。

`cef_osr_probe` 的 AppKit 实现只在 macOS 编译；其他平台保留会明确报告不支持的入口。macOS 仍运行同一个真实 OSR 探测实现。

桌面 E2E 显式启动隔离应用并等待本机 DevTools 端点，再让 Edge WebDriver 附加。每个会话与应用重启使用独立浏览器 profile，保留同一个应用数据目录；全部原有规格仍执行。该流程适用于 Pet 与 Workbench 多 WebView 应用，避免驱动自行启动时找不到 `DevToolsActivePort`。

跨多个设置页与真实 VRM/VRMA 预览的四个视觉场景使用 90 秒总预算，容纳 hosted Windows 软件渲染的加载时间，包括 Motion Library Lab 的完整手指诊断与截图；每个断言仍使用原有时限，截图比较与功能步骤完整保留。

OSR 尺寸更新先通知屏幕缩放变化，再通知视图尺寸并请求新帧，覆盖逻辑尺寸相同的 Retina 到 1x 切换。CEF smoke 仍校验帧的实际物理尺寸，超时会记录最近帧尺寸。

Windows 路径 smoke 将刚创建的临时测试目录所有者设为当前用户，满足沙箱的既有前提。Hosted runner 可能以管理员运行并默认使用 Administrators 组作为所有者；生产的用户所有权、NTFS、重解析点与硬链接校验继续执行。

Hosted Windows 的桌面任务先构建带 `desktop-e2e` feature 的 debug 可执行文件，记录并校验 SHA-256，再以临时标准用户运行全部七个规格。子进程会断言自己没有管理员权限；测试账户和临时目录在结束时清理。该入口仅允许在 GitHub Actions 使用，编译阶段与交互验收使用同一个专用构建目录。此调整也用于验证上游记录的 [WebView2 elevated remote-debugging 问题](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5640) 是否解释 hosted runner 的端口不可达现象。

Git 能力探测在新建临时目录中使用等价的 Windows 路径表示，避免 Git 配置解析器误读 `\\?\` 前缀。可执行文件身份与 lease 仍使用原有规范路径；注册表/标准安装位置的测试单独排除测试覆盖路径，并继续验证实际 Git 操作。探测实现拆为独立模块，遵守文件长度限制。

WebView2 附加与重启显式使用 [WebDriver Classic](https://webdriver.io/docs/capabilities/#wdioenforcewebdriverclassic)，避免 WebdriverIO 自动切换 BiDi 后出现失效会话和悬挂的 `script.callFunction`；全部窗口切换、重启和原有功能断言仍执行。

工作区 Rust 测试将新建 TempDir 的目录树所有者设为当前用户，覆盖 Git fixture 元数据；该入口仅在测试模块可用。完整桌面 E2E 同样加载 Avatar V5 的用户动作 fixture。Run 审阅前先关闭覆盖时间线的宽文件面板，Motion Lab 等待实际切换诊断完成后再运行矩阵。失败时保存当前 HTML 与隔离测试会话快照，帮助区分选择器、投影和实际工具执行失败。

Hosted CI 的视觉任务使用一个 Playwright worker，避免多个真实 VRM 软件渲染场景争用资源。显式 Axe 审计只在引擎报告“already running”时等待 Storybook 的在途审计结束；实际违规结果和其他异常仍立即交给原断言处理。Storybook 的 `a11y.test: error`、WCAG 标签、截图基线与比较阈值保持生效。

真实 Worker 进程和 mock provider 驱动的集成测试也复用 TempDir 所有者 helper，确保 Hosted 管理员 runner 满足工作区的个体用户所有权前提。集成测试仍验证真实文件变更、审批、审计与重启持久化；仅修改刚创建的测试目录。

终端生命周期检查完成后再启动待重启的审批 Run，避免无关交互消耗既有工具时限。动作 E2E 在同一页面采样中确认 ambient 动作标识与 action slot，再校验回到 waiting；Skill 菜单通过已有键盘激活路径操作。动作特征缓存 IPC 使用两秒预算，超时按现有缓存失败路径从不可变 VRMA 重建；真实动作分析和切换准入阈值继续执行。

终端 fixture 显式进入系统 PowerShell，再发送 PowerShell 语法，兼容运行时选择 CMD 的标准用户环境。Git 首次提交继续经过实际界面按钮的 DOM 禁用状态校验。内容 SHA-256 相同的派生动作复用父 VRMA 的解析结果，编译时仍应用派生时间范围，并维持现有 LRU 容量限制；共享源加载失败时清理父动作与派生动作的失败缓存，允许重试。真实 VRM/VRMA 集成测试校验一次源读取、派生首尾姿态和失败后重试。

锁定版 Kobalte 在弹层滚动查找中可能重复当前滚动容器并冻结页面。通过 pnpm 依赖补丁让查找逐级向父元素推进，保留滚动锁定与键盘交互；生产浏览器测试覆盖创建 Skill 后的鼠标和键盘菜单操作、文件对话框及设置页切换。详见 [依赖补丁说明](dependency-patches.md)。

关闭终端时立即失效共享启动缓存，防止面板重新打开后复用已终止的进程；回归测试覆盖关闭最后一个终端、卸载并重新打开面板。原生终端 fixture 按进程只进入一次 PowerShell。Skill 规格等待实际保存完成后再切换文件，保留未保存确认和冲突校验。

原生动作规格在页面内记录短暂的 ambient action 入槽事件，再由驱动读取记录，避免两个轮询之间漏掉一次性动作。自动动作仍须是 once，并实际恢复 waiting；等待预算容纳 25 秒最大调度延迟和冷资源载入。Motion Lab 初始化采用 75 秒冷加载预算，完整 12 组矩阵及切换准入阈值继续执行。

Windows 桌面 CI 的常规构建和带 E2E feature 的构建顺序共用专用 Cargo target，复用依赖编译产物；仍执行两种构建并校验最终 E2E 可执行文件的 SHA-256。专用目录与日常构建目录独立。

恢复系统 Git 后，原生规格等待真实能力探测和首次提交表单完成；stdio MCP 规格等待新建服务被实际选中并已启用，再读取工具列表，避免旧服务详情仍在渲染时继续。

动作源解析、协议读取、字节读取和 GLTF 解析分别记录阶段并使用 20 秒预算，失败后释放共享源缓存以便重试。协议读取遵守原生响应的 no-store 策略；解析结果仍由有容量限制的内存缓存复用。Motion Lab 将最后阶段写入诊断属性，失败快照可区分卡住的阶段。真实资源的 Chromium 及隔离双窗口 Tauri/WebView2 探针均通过完整 12 组矩阵；完整桌面应用的 GitHub 验收仍以必需 CI 为准。

原生 avatar/motion 协议使用异步响应，目录锁和文件读取在后台阻塞线程中处理；窗口范围、GET 限制、资源 ID 校验、目录边界和返回的 no-store 策略不变。此调整用于处理完整桌面应用中已定位到 fetch 阶段的等待，仍需完整平台 CI 验证。

PowerShell PTY 测试等真实提示符就绪后再发送命令，保留 cwd、前台 Ctrl+C 与退出检查。Pet 规格等待权限保存成功，并在页面内记录短暂回复；ambient 规格记录同一次动作的入槽与恢复，避免下一次自主动作开始后漏采 waiting 状态。
