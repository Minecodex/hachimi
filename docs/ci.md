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
