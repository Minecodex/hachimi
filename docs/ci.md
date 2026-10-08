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
