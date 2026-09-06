## Hachimi beta-1.1

这是首个同时提供 Windows x64 和 macOS Apple Silicon 安装包的 Beta 预发行版。

### 下载

- `Hachimi_1.0.0_x64_en-US.msi`：Windows MSI 安装程序
- `Hachimi_1.0.0_x64-setup.exe`：Windows NSIS 安装程序
- `Hachimi_1.0.0_x64-portable.zip`：Windows 便携版
- `Hachimi_1.0.0_aarch64.dmg`：macOS 13+ Apple Silicon 安装镜像
- `SHA256SUMS.txt`：本次发布全部二进制产物的 SHA-256

### beta-1.1 重点更新

- 新增 macOS ARM64 桌面、Seatbelt Sandbox、CEF 浏览器、终端、语音和 Computer Use 基础支持。
- 新增统一系统运行时发现：启动时发现并验证 Git 与默认 Shell，安装或修复后可在设置中重新检测，无需重启应用。
- Git 改为能力探测，不再以统一最低版本拒绝 Apple Git；Apple Git 2.39.5 可完成空首次提交。
- Workbench、Sandbox ACL 和 Workspace Worker 使用同一经过验证的绝对 Git 路径，并检测运行期间的可执行文件替换。
- 修复空目录执行 `git init` 后首次提交误报 `sandbox_acl_prepare_failed` 的问题。
- Git 不可用、探测超时、能力不足或运行时漂移时，界面会显示明确原因并禁用相关操作。

### 安装提示

- Windows 安装包暂未代码签名，Windows Defender/SmartScreen 可能显示安全提示。
- macOS DMG 使用 ad-hoc 签名、尚未 Apple 公证；首次打开可在 Finder 中右键 Hachimi，选择“打开”。
- 这是 Beta 版本，请在使用项目写入、Git 操作和桌面控制前备份重要数据。
- Linux 暂不支持。
- 官方包包含默认 VRM，其许可禁止个人及企业商业使用，因此官方安装包仅用于非商业场景。Hachimi 源代码仍采用 Apache-2.0；商业发行前需要替换默认模型或另行取得授权。
