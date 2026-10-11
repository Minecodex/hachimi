# Hachimi 0.3.0 发布 Gate

本文说明 `v0.3.0-alpha.8 → v0.3.0-rc.1 → v0.3.0` 的可执行 Gate、受保护配置和证据边界。当前只完成了 Gate/发布代码与本地验证；真实外部环境和两类 Windows Runner 尚未执行，不能据此创建 tag 或声明发布完成。

## 身份与“租户”边界

Hachimi 保持本机单用户产品，不增加 Hachimi 账号、登录、用户租户、云端控制面或远程多租户体系。文档和配置中的 `tenantId`、`corpId`、组织或“企业三环境”只表示企业微信、钉钉、飞书各自返回的外部组织标识，用于凭据绑定、事件验签和跨组织隔离。

OpenAI、Forge 与企业平台凭据只存 Windows Credential Manager。Staging JSON 只允许 endpoint、模型、仓库/组织、测试 Peer/Group、文件路径和 `secretRef`；任何 `apiKey`、`token`、`password`、带用户名密码的 URL 都会以稳定错误码拒绝。

## 命令与证据

| 命令                                                                       | 用途                                                                            | 当前状态                         |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | -------------------------------- |
| `corepack pnpm release:version-check`                                      | 校验 Cargo/package/Tauri 版本、Apache-2.0 元数据、NOTICE 边界及安装包资源声明   | 本地可执行                       |
| `corepack pnpm release:artifact-manifest -- target/release-candidate`      | 对 MSI、NSIS、便携 ZIP、来源 registry、LICENSE/NOTICE 生成 SHA-256              | 本地可执行                       |
| `corepack pnpm release:artifact-verify -- --root target/release-candidate` | 下载后重新哈希候选，拒绝 manifest、commit、version、来源或许可漂移              | 本地可执行                       |
| `corepack pnpm test:staging:openai`                                        | 真实 OpenAI 与同次确定性故障 conformance                                        | 环境阻塞                         |
| `corepack pnpm test:staging:forge`                                         | Git Host 与五个 Forge 环境                                                      | 环境阻塞                         |
| `corepack pnpm test:staging:enterprise`                                    | 三个外部企业组织的 REST/Stream/长连接验证                                       | 环境阻塞                         |
| `corepack pnpm test:staging:channels`                                      | 钉钉、飞书、企微 AI Bot、企微自建应用、微信 iLink 五个平台的文本/媒体/恢复 Gate | 环境阻塞                         |
| `corepack pnpm release:evidence:verify`                                    | 聚合六类原始 `summary.json`，fail closed                                        | 本地聚合逻辑已验证；真实证据缺失 |

证据位于 `target/release-evidence/<run-id>/`。只保存 schema、Gate 状态、版本、commit、候选/来源哈希、脱敏环境指纹、稳定检查 ID、详情哈希、时间和稳定失败码；不保存 Secret、原始消息、隐藏 reasoning、附件正文、完整远端响应或本机敏感路径。

## OpenAI 配置

环境变量 `HACHIMI_STAGING_OPENAI_CONFIG` 指向 JSON：

```json
{
  "schemaVersion": 1,
  "gateKind": "openai",
  "environmentFingerprint": "protected-openai-staging",
  "secretRefs": ["credential-manager:provider:default"],
  "baseUrl": "https://api.openai.com/v1",
  "chatModel": "<protected-chat-model>",
  "responsesModel": "<protected-responses-model>",
  "embeddingModel": "<protected-embedding-model>",
  "requireReasoningSummary": true,
  "requireRemoteCompaction": true,
  "overflowProbeChars": 1200000
}
```

真实测试通过产品 `OpenAiCompatibleRuntime` 和 Provider registry 覆盖 Chat/Responses/Embeddings、stream、Tool、usage、取消、Provider 错误、overflow、远程压缩与公开 summary。Capability drift、畸形响应、超时、本地 fallback 和隐藏 reasoning 过滤由同一 Gate 先运行确定性 conformance，再与真实运行写入同一份 summary。

## Forge 配置

环境变量 `HACHIMI_STAGING_FORGE_CONFIG` 指向包含五项 `repositories` 的 JSON。`platformLabel` 必须分别为 `github`、`gitlab`、`gitee`、`gitea`、`forgejo`；Gitea 与 Forgejo 都使用 `forgeKind: "gitea_forgejo"`，但必须是两个独立环境。

每项仓库配置必须包含：

```json
{
  "platformLabel": "github",
  "forgeKind": "github",
  "apiBaseUrl": "https://api.github.com/",
  "faultApiBaseUrl": "https://<protected-fault-proxy>/github/",
  "owner": "<test-owner>",
  "repository": "<test-repository>",
  "remoteUrlHash": "<sha256>",
  "secretRef": "release-github",
  "sourceRef": "hachimi-gate/<run-id>/change",
  "targetRef": "main",
  "expectedCommitOid": "<40-hex-oid>",
  "mergeSourceRef": "hachimi-gate/<run-id>/merge",
  "mergeCommitOid": "<40-hex-oid>",
  "checkoutPath": "<protected-disposable-checkout>",
  "remoteName": "origin"
}
```

Git fetch/push 通过 `WorkspaceHostClient` 与固定解析的系统 Git 执行，凭据由 GCM/SSH Agent 提供。Forge token 使用 Credential Manager。create/query/update/close/merge 使用产品 adapter；确定性 ledger 测试与真实 mutation 属于同一 Gate。传入的 Forge Approval 会重新校验 Session、Run generation、Tool call、参数哈希、一次性 scope、解析主体和过期时间，旧 Approval 不能复用于 merge。

每项还必须提供 `faultApiBaseUrl`，指向受保护的透明故障代理。代理把 mutation 完整转发到原 Forge 后丢弃响应，但继续放行只读查询；adapter 随后按 source/target、可见字段、状态和 source commit OID 做远端 reconciliation。只有精确匹配才确认成功，Create/Close/Merge 的 staging 测试还会断言本次结果确由未知响应恢复，避免把普通成功响应误算为故障验证。

## 企业平台配置

环境变量 `HACHIMI_STAGING_ENTERPRISE_CONFIG` 指向三个 API 组织 `connections`：

```json
{
  "schemaVersion": 1,
  "gateKind": "enterprise",
  "environmentFingerprint": "protected-enterprise-organizations",
  "secretRefs": [
    "keyring:connector:release-wecom",
    "keyring:connector:release-dingtalk",
    "keyring:connector:release-feishu"
  ],
  "connections": [
    {
      "platform": "wecom_app",
      "accountId": "release-wecom",
      "credentialRef": "keyring:connector:release-wecom",
      "departmentId": "<test-department>",
      "peerId": "<test-peer>",
      "groupId": "<test-group>",
      "expectInboundEvent": true,
      "callbackPublicUrl": "https://<protected-reverse-proxy>/v1/channels/wecom_app/release-wecom/callback"
    }
  ]
}
```

钉钉、飞书对应 `platform` 为 `dingtalk`、`feishu`，三个 connection 的 `expectInboundEvent` 都必须为 `true`。企微自建应用 callback 使用账户级 `/v1/channels/wecom_app/{accountId}/callback`，不接受 query 形式的旧路由。REST Gate 验证 API 和主动消息；五平台 Channel Gate 另外验证真实 callback/WS/长轮询事件、稳定地址、文本/图片/文件、重启恢复和凭据撤销。

钉钉/飞书受保护入站 fixture 同样必须带结构化 mention 和允许类型附件；真实下载使用产品 `EnterpriseApiClient`、25 MiB 上限和远端 ID。MIME/magic、拒绝 HTML/可执行文件、Artifact fencing、重复下载和未知结果仍由同一 Gate 的确定性 `PluginHost` 测试验证，fixture 不替代真实传输。

## 五平台 Channel 配置

环境变量 `HACHIMI_STAGING_CHANNELS_CONFIG` 必须包含恰好五个 `connections`，`providerId` 为 `dingtalk`、`feishu`、`wecom_ai_bot`、`wecom_app`、`wechat_ilink`。每项的 `credentialRef` 固定为 `keyring:integration:{providerId}:{accountId}:primary`，图片和文件 fixture 只允许受保护 staging 路径，所有 `expect*` 能力必须显式为 `true`。iLink 只允许 DM，并额外提供按 Conversation 保存的 `conversationSecretRef`；企微自建应用提供账户级 callback URL 和反代捕获的加密 callback fixture。配置校验会拒绝 Secret、带凭据 URL、旧 `wecom` provider 和旧 callback path。

`corepack pnpm test:staging:channels` 先运行五平台 deterministic fixture，再运行被 `#[ignore]` 保护的真实测试。测试通过产品 transport 发送文本、图片、文件，等待带结构化 parts 的真实入站，重建连接后再次投递，并短暂撤销/恢复临时 Credential Manager 项验证撤销行为。没有五平台受保护账号、外部消息和反代 fixture 时，Gate 必须失败并在证据中保持环境阻塞，不能用本地 fixture 标记真实通过。

这里的三个 connection 是三个外部平台组织，不是 Hachimi 租户。真实入站 callback、mention、附件、限流、凭据撤销和重启 reconciliation 在受保护环境不存在时保持“真实环境待验证”。

## Windows 与发布

`windows-release-gate.yml` 在 GitHub 托管 Windows 上复用同一提交的完整 CI，再构建一次 MSI、NSIS 和便携 ZIP。不可变 manifest 绑定实际版本、commit 和三类包哈希；包校验、源码与许可检查仍执行。Actions 只证明托管软件及候选制品范围，不证明真实安装升级身份、签名浏览器身份或外部组织接入。

真实 standard-user/elevated 安装升级保留 `scripts/test-windows-release.ps1` 等独立入口；下载或复制同一候选后重新校验版本和哈希。它们需要实际交互用户、安装环境及前一版本，后续独立环境具备时执行。原有各安装路径中的 LICENSE、NOTICE、默认 VRM/动作和语音资源校验保持有效。

`External Staging Gate` 和 `Publish Verified RC or GA` 已从 GitHub Actions 移除，不再要求 Actions Runner 标签、Environment、外部平台 Secret 或专用主机。前面的 OpenAI、Forge、企业及 Channel 协议测试与本地 staging 命令仍保留；缺少真实环境时明确记录待验证。

六类独立证据仍由同一校验器检查候选归属、失败/跳过、实际哈希、源码漂移及证据时效：

```sh
node scripts/release/verify-evidence.mjs --root target/independent-release-evidence --required openai,forge,enterprise,channels,windows_standard_user,windows_elevated --artifact-manifest target/release-candidate/artifact-manifest.json --expected-version YOUR_VERSION --expected-commit YOUR_COMMIT --output release-manifest.json
```

后续正式环境认证与 RC/GA 发布使用独立的证据和操作流程。没有这些环境证据时，不把候选制品或受控 CI 的成功作为真实环境认证。

`publish-alpha-prerelease.yml` 继续只接受已成功的 Windows 候选构建，核对源码 CI、commit/version/hash/source/license 后创建不可变 alpha prerelease；不要求专用发布 Environment。跨平台 beta 同样先通过完整托管 CI 再生成两种原生候选。alpha/beta 不携带六类真实环境 Gate 的通过结论，既有 tag 不得覆盖。
