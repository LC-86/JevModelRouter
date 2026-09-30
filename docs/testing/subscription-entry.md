# 订阅服务商入口与默认拒绝（Issue #12）

本票建立订阅服务商进入既有配置、服务商面板、后端与公开调用入口的边界。它不包含真实登录、模型目录、
额度读取或订阅生成传输；这些由后续子票实现。当前两家订阅服务商的生成能力一律默认拒绝。

## 领域与配置

- `ProviderKind` 新增 `codex_subscription` 与 `grok_subscription`。它们沿用稳定的服务商标识、名称与启停，
  但没有 `base_url`、`api_type` 与 API 凭据：订阅身份与 API 身份分开保存，旧 API 配置与模型原样保留。
- `AppConfig.subscriptions` 按服务商标识保存活动连接（每家一个），包含连接世代、授权状态、已核实身份与
  一份只读证据。缺少该字段的旧配置可直接读取，迁移与往返保存有单测覆盖。
- 服务商删除、标识重命名与类型转换会同步连接：重命名保留世代与身份，删除或转为 API 服务商即放弃订阅身份。

## 适配边界

`subscription::SubscriptionAdapter` 是唯一的订阅边界，包含三类只读读取（连接状态、模型目录、额度证据）
与绑定服务商、账号世代、模型、协议的生成交接。实现只在构造 `ConfigStore` 时注入：

- 生产构造只安装 `UnavailableAdapter`：只读读取如实返回不可用，生成一律拒绝。
- 配置、界面与环境变量都没有把它换成替身的开关；替身只存在于测试代码中（`refresh_tests`）。

> 后续变更（#13）：该票之后，生产改在 `lib.rs` 构造 `ConfigStore` 的**唯一一处**注入真实的
> `CodexAdapter`（专用官方 Codex 辅助进程），`UnavailableAdapter` 降级为不可用与测试路径。注入点仍写在
> 代码里，配置、界面与环境变量都没有替身开关；`--autojev-helper` 只存在于 `isolation-check` 构建，
> 且额外要求已进入隔离模式。因此隔离验收里 `adapter_available` 的期望由 `false` 改为 `true`，
> 而真实生成依旧全部拒绝（见 `docs/testing/codex-login-handoff.md`）。

只读刷新（`refresh_subscription`）绑定当前连接世代：读取期间换号、删除服务商或世代变化时，迟到的结果
被整体丢弃，不写入新世代。刷新不检查网关暂停，因此生成暂停时仍可恢复必要的只读依据。

状态、目录、额度三项读取彼此独立（#15）：`status` 读取失败才返回 `Err` 且不改写证据；`models`/`quota`
之一失败仍按本轮状态规则写回并返回 `Ok(snapshot)`，由界面上的 `stale`/`failed`/`history` 表达失败，
不用错误弹窗掩盖保留的历史数字。目录读取失败且此前已有已核实目录时标 `stale` 并原样保留旧列表，
此前没有目录则标 `failed` 且列表为空；额度读取失败标 `failed` 并保留上一次同账号的桶与 `observed_at`，
只有此前确实成功读过才 `history = true`（首次读取即失败是 `history = false`、`observed_at` 为空、无历史文案），
绝不把临时失败伪装成可用或余额为零。额度桶与额外用量 credits 分别表达，不换算金额或 token、不跨桶相加；
根层许可为权威，但桶内显式 `false` 不被根层 `true` 覆盖（fail-closed）。

一份证据同时绑定连接世代与已核实账号，并记录辅助进程版本与专用账号路径；任意一项不符，整份证据作废，
不会降级成“部分可用”。真实目录与真实额度仍属 #25 手测；本票的隔离证据边界与覆盖矩阵见
[Codex 模型目录与额度只读展示](codex-catalog-quota.md)。

## 统一准入

`subscription::admit_model` / `admit_target` 是网关、Debug、服务商测试、模型测试与手动测速共用的准入：

| 结果 | code | 类别 |
| --- | --- | --- |
| 服务商或模型停用 | `provider_disabled` / `model_disabled` | Disabled |
| 未连接、等待授权、授权过期、身份未核实、无当前世代证据 | `not_connected` / `authorization_pending` / `authorization_expired` / `identity_unverified` / `evidence_missing` | NotConnected |
| 账号不具备模型资格 | `model_not_eligible` | NotEligible |
| 能力未验证或不支持 | `capability_unverified` / `capability_unsupported` | Capability |
| 额度依据未知、陈旧、读取失败或无机器接口 | `quota_unknown` / `quota_stale` / `quota_failed` / `quota_unsupported` | Quota |

网关按客户端协议返回结构化错误体，并带上 `code`、类别、恢复动作、`retryable` 与
`x-autojev-subscription-denial` 响应头；被拒绝的订阅模型不进入自动选路候选，固定直调返回该目标的具体原因。
服务商测试与模型测试返回同样的原因文本，测速在派发前拒绝并记录失败样本。

订阅模型不参加自动测速（`performance::due_models` 排除订阅服务商）。手动测速仍可指定，但同样先过准入。

## 界面

服务商面板新增“订阅”列：显示真实连接状态、已核实身份（缺失即未知）、已发现模型数量与当前拒绝原因；
订阅服务商提供“刷新只读状态”，不提供 API 密钥、Base URL 与测试模型字段。模型列表为订阅模型显示
未验证／已验证／不支持的能力标记，默认即未验证。

## 验证

```sh
pnpm release:check
pnpm test
pnpm build
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
```

Rust 侧覆盖：准入拒绝矩阵与稳定 code、世代不符时证据整体作废、账号不符时证据整体作废、两家服务商证据
互不借用、旧配置无损读取、生产无替身开关、只读刷新在网关暂停时的可用性与迟到结果丢弃、三项读取彼此
独立（`status` 失败返回 `Err` 且不改写，`models`/`quota` 失败按状态规则写回并返回 `Ok`）、目录读取失败
保留已核实目录并标 `stale`、额度读取失败保留上次桶与 `observed_at` 并标 `failed`（确有历史数字/时间时才
`history = true`，首读即失败为 `history = false` 且无时间）、缺失与越界字段
分别记录（越界值不截断、不写成 0）；以及监听网关上的端到端断言：三种下游协议的固定订阅目标与被排除的自动
选路都不会让回环替身收到任何请求（`AtomicUsize` 计数为 0），而 API 目标照常工作。

原生隔离桌面验收（`pnpm test:isolated`）在真实界面中添加订阅服务商与订阅模型，检查行内显示“未连接”、
服务商测试与只读刷新返回原因、手动测速报告原因，并在替身侧断言从未收到订阅模型的生成请求。

#15 追加一次 `--autojev-login-check catalog` 运行：登录后按替身队列逐个排练多桶/单桶/缺字段/越界/拒绝/
许可缺失/读取失败，断言同时取自界面稳定文本与后端 snapshot；覆盖矩阵见
[Codex 模型目录与额度只读展示](codex-catalog-quota.md)。

这些证据只证明默认拒绝与服务商入口的受控行为，不证明真实授权、身份、目录、额度或订阅生成能力。

Grok 订阅的登录、取消、退出与换号交接见 [Grok 订阅登录、退出与换号手测交接](grok-login-handoff.md)。
