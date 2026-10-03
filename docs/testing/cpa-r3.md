# R3：CPA 订阅连接与身份世代

现行规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，实施票 [#54](https://github.com/LC-86/JevModelRouter/issues/54)，基线 `5dd59847cc3afb998a6b747fa5153dfef6394d1a`。这是隔离替身交付；真实 OAuth 登录、真实账户/额度查询和真实模型请求均为 **0**。R7 承接真人验收，R6 承接可交付服务配置。

## 行为与边界

桌面服务商页有独立 CPA 连接面板，公开操作包含添加、发起、检查等待/成功/失败、取消、退出、换号、读取目录、选择和明确重新绑定。启动不会登录、查额度或生成。普通构建没有已配置的 CPA 授权服务，界面明确显示未配置；只有 `isolation-check` 在专用临时 root 和显式 `--autojev-cpa-auth-check` 下注入虚构回环管理服务。测试只展示 `.example.invalid` 授权链接，不打开真实授权页面。

`AppConfig.cpa_subscriptions` 保存来源连接与非秘密账号/套餐、连接实例和世代；`cpa_model_bindings` 独立保存固定目标的来源、实例、世代、账号、套餐及原模型 ID。Provider/Model UUID 和 `autojev/model/<UUID>` 沿用已有接口。账号/套餐改变、取消和退出递增世代，旧结果整体丢弃；发现目录不改变保存的模型绑定。新账号必须明确重新绑定，同名 API 不接替。临时读取失败保留历史目录与配置并标陈旧；已观察的身份变化独立于目录读取提交，不因目录失败保留旧身份。

CPA 管理 key、会话 state 和 HTTP 客户端只在后端；桌面只取得授权链接与非秘密投影。只使用固定的 v8 管理操作，没有 `credentials/download`、上传 auth-file、任意 `requests/api-call` 或 token 刷新实现。OAuth/token 存储及刷新属于 CPA；Jev 不读取日常 Codex/Grok CLI 凭据。退出先提交本地拒绝，再取消本连接登记的 session 或删除本连接已认领的 credential reference。清理失败保留清理引用供明确重试，连接保持不可调用；不遍历/删除其它实例凭据，也不终止其它 CPA/CLI 进程。

本票不生成订阅资格、额度、协议能力或整次调用仅用订阅内权益的证据。网关、Debug、服务商/模型测试和测速沿用共同准入；CPA 目标明确返回 `cpa_not_connected`、`cpa_target_identity_changed` 或 `cpa_qualification_unknown`，不走旧 CLI 派发。仅有登录/目录/选择仍零派发。

## 固定接口与逐来源状态

固定源码 `e2bff0107bb307337aaa19018ccddd55f64253d5`，v8 来源、制品与 MIT 许可参见 [R1](cpa-r1.md) 和 [manifest](../../scripts/cpa-artifact.json)。源码查证位置：`internal/api/server_management_v8.go`、`auth_files_v8.go`、`auth_files_provider_oauth.go`、`auth_files.go`。这些是固定版本的接口事实，不是账号资格证明。

| 接口 | 本票用途与证据 |
| --- | --- |
| GET `oauth/auth-url?provider=codex` | 取得 CPA 创建的 URL/state；虚构流程通过 |
| GET `oauth/status?state=…` | 区分 wait/ok/error；虚构流程通过 |
| DELETE `oauth/session?state=…` | 取消登记的 session；不自建 OAuth，迟到结果不能恢复连接 |
| GET `credentials` | 读取单凭据专用 profile 的非秘密元数据；严格过滤账号/套餐字段，忽略原始额外字段 |
| GET `credentials/models?name=…` | registry 发现条目；**不**报告 eligible=true |
| DELETE `credentials?name=…` | 只清除已认领引用；不声明远端账号撤销 |

固定 `credentials` 列表中的 Codex `id_token` 字段是 CPA 提取的 claims 对象；Jev 仅取 `chatgpt_account_id` 与 `plan_type`，不取 JWT 原文、不推断邮箱/默认套餐。目录属于凭据 registry，不证明当前套餐可调用。没有建立普通模型 HTTP 的 OAuth 固定账号保证，也没有启用订阅生成路径。

| 目标订阅来源 | 固定 CPA 授权入口 | Jev R3 状态 | 模型/资格/费用 |
| --- | --- | --- | --- |
| Codex | `StartOAuthV8` 分派 `RequestCodexToken` | 支持已查证管理契约的隔离替身闭环；真实授权待 R7 | 目录替身通过；真实调用、定向 OAuth profile、资格和费用均待验证 |
| xAI | 分派 `RequestXAIToken` | CPA 源码有入口；Jev 待接入/验证 | 未验证，不借用 Grok CLI 资格 |
| Claude、Antigravity、Kimi、kimi-ai、Devin、Meta | 固定 switch 有对应分派 | CPA 源码有入口；Jev 本票不支持这些连接操作 | 未验证，不从 CPAMP provider 枚举推能力 |
| 其它/插件来源 | 取决于实际插件 host；未知来源返回 provider_not_found | 未验证/不支持自动接入 | 不公布模型协议或账号权益支持 |
| 旧 Codex/Grok CLI | 保留既有历史/兼容路径 | 不自动迁移其凭据或改绑到 CPA | 旧证据不自动移用 |

## 验证与存储口径

最高层接缝沿用规格已确认的本地网关、临时 SQLite、公开桌面操作和回环管理替身。已观察：原生 webview 按钮闭环、A→B 换号后的旧固定目标拒绝、失败陈旧目录、显式重新绑定、两次原生进程的保存/重读和新的 run ID 报告。图片为本任务启动的原生窗口，按其 PID 限定截图，未截取其它应用：[原生截图](../screenshots/cpa-r3-native.png)。

2026-10-03 的当前实现通过前端 96 项、Rust 505 项（其中新增 CPA 行为 6 项与界面投影 1 项），以及构建和 release 检查。[原始桌面报告](cpa-r3-evidence.json) 保留首轮 `af937f04-587e-4f7c-b990-9050f5424383` 与重启 `a31dff76-a73b-4f61-a667-7c9776aca1da` 两个 run ID；两轮 `ok:true`、同一 provider/model UUID 与世代 8，接收端模型请求、凭据下载及额度查询均为 0。截图来自后一次重启，服务不自动重连。取消迟到授权、换号迟到目录、账号变化且目录失败和待授权进程重启分别由公共管理/临时 DB 测试覆盖。

隔离 SQLite/重启与应用专用临时路径已验证；**真实 macOS Keychain、CPA OAuth auth-dir 写入/刷新、真实账号和真实额度尚未验证**。本票管理替身使用虚构元数据，未安装真实服务或凭据。CPA R1 的实际 API 定向结果不推广到 OAuth。

```sh
TAURI_DEV_HOST=127.0.0.1 pnpm build
TAURI_DEV_HOST=127.0.0.1 pnpm test
pnpm release:check
CARGO_TARGET_DIR=/absolute/owned-target cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --lib
CARGO_TARGET_DIR=/absolute/owned-target cargo build --locked --offline --manifest-path src-tauri/Cargo.toml --features isolation-check
node scripts/check-cpa-subscriptions.mjs /absolute/owned-target/debug/autojev --capture
```

桌面驱动保留 `first.report.json`、`reload.report.json`、`receiver-records.json` 和本轮 run ID 截图。两份报告均需 `ok:true`，保存的 provider/model/generation 必须一致。缺报告、锁屏、超时均不计通过。清理仅针对该驱动启动的桌面进程组、Vite 和回环服务；截图失败如实输出，不改变产品验证报告。

合入顺序由父线程协调 R2 后 R3，合入后需重新验证共有接缝。共享字段集中在 AppConfig/Snapshot 的独立 CPA map，服务商、模型 UUID 与网关准入沿用已有接口。
