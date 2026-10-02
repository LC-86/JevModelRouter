# Grok 目录与额度适配状态（Issue #16；设计契约已撤销）

**当前生产行为：账号身份、模型目录、订阅池和额外用量均为 Unknown。** 本文件旧版定义的 `account --json`、`models --json`、`usage --json` 与 `account/catalog/quota` JSON 行事件，是未获上游支持证据的应用侧假设，已从生产适配路径撤下。不得手动尝试这些命令，也不要将旧版 JSON schema 当作 Grok CLI 契约。

## 当前实现

- 生产 `GrokCliAuth::begin` 在准备 helper home 或创建进程前拒绝登录，错误码为 `grok_auth_unverified`；生产 helper 状态报告 `login_supported=false`。
- 生产 Grok 只读状态不创建 CLI 子进程。身份状态不可核实时返回明确错误；模型目录及额度证据保持 `Unknown`，缺少的身份、目录、窗口、许可和额外用量字段以 `missing_fields` 记录。
- UI 未验证身份显示 `Unknown`。未知订阅许可不通过准入；未知整次调用额外消费限制时拦截派发。Unknown 不代表余额为 0、无额度或无额外费用。
- 本地 Rust fake-helper 单测仍可用显式测试注入走旧映射 fixture，用于检查防回归和 UI 呈现。这只验证 AutoJev 的本地映射逻辑，不代表生产接口或账号行为成立。

## 接口核对结果

本机 `@xai-official/grok` 1.0.44 的只读帮助显示：`account` 不可用；`models --help` 没有 `--json`；`usage` 接受本地 session ID，统计 session token/cost，不是订阅额度或 credits API。此句记录当时的 CLI 帮助核验；2026-10-02 后续另做了一次有界 ACP v1 initialize + authenticate(cached_token) 现有会话检查，成功但未检查或保留账户身份，未发 session/new 或 session/prompt，详情见 [Grok ACP 只读路径与来源阻断](grok-acp-readonly.md)。

官方 CLI 文档只把 `models` 描述为可用模型列表；它没有确认本机 1.0.44 对候选机器接口的支持。npm 注册表的[精确 1.0.44 记录](https://registry.npmjs.org/@xai-official/grok/1.0.44)返回 HTTP 200 且无重定向，`_id` 与 `version` 均为 `@xai-official/grok@1.0.44` / `1.0.44`，`gitHead` 为 `5b807183dd7978a460f309132cf0d1183d743526`，没有 `repository` 字段。本机安装包清单没有 `gitHead` 或 `repository` 字段；CLI 历史 `--version` 自报 build ID `5b807183dd79` 与注册表 `gitHead` 前缀相同。对该完整 SHA 的公开 GitHub 仓库查找返回 HTTP 422，只能说明这次查找没有解析该 SHA，不能证明不存在其他源码映射；本机二进制的精确字节仍未映射到可核验的官方源码或发布。较新的官方公开快照 [`2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`](https://github.com/xai-org/grok-build/commit/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8) 的 [`xai-grok-shell/Cargo.toml`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/Cargo.toml) 声明版本为 1.0.45，而不是本机 1.0.44。该公开快照在 [`billing.rs`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs) 中有 `x.ai/billing` 和 `x.ai/auto-topup-rule` 候选 handler；它不证明 1.0.44 的 ACP wire method 或响应契约。

较旧固定快照 1.0.16 中这些候选方法的语义也不足以放开生产准入：`auth/info` 的 `current_or_expired` 状态不证明登录当前有效；`auth/check_subscription` 会刷新 JWT，是有状态网络操作，本只读验收不得调用；`models/list` 不证明模型具备订阅内资格。该快照的 `billing` 与 `auto-topup-rule` 可读用量、周期、预付余额、按需 cap/已用量与远端自动充值设置，但 `on_demand_enabled` 是远端设置状态，不是阻止消费的执行保证；检查到的 GET 方法也没有提供“整次调用不扣额外 credits”的保证。官方 [Usage & limits FAQ](https://docs.x.ai/grok/faq#usage--limits) 说明订阅周额度用尽后会使用 Extra Usage credits；关闭自动充值不等于已有余额不会被消费。因此，本地版本是否支持这些方法、身份有效性、模型资格和禁止额外消费的保证仍为 **Unknown**。本机 `grok account` 不可用、`models --help` 无 `--json`、`usage` 读取本地 session token/cost 的事实不变。CLI headless/ACP 的 `session/prompt` 会生成内容，不能用于只读探测。

## 验收

运行 `pnpm test:grok-contract` 只验证本地 fake-helper 合同拒绝、Unknown 展示和前端状态文本。它不运行 Grok CLI，不证明真实身份、目录、额度、Extra Usage 权限或费用上限。安全人工验收入口和当前阻塞项见 [grok-hand-run.md](grok-hand-run.md)；结果记录模板见 [grok-hand-run-result-template.md](grok-hand-run-result-template.md)。

当前生产面板显示 source/version gate；[ACP 只读路径与来源阻断](grok-acp-readonly.md)记录候选方法和版本映射缺口。本次没有实现或模拟 ACP wire 方法。Rust 账单 DTO 解析器仅在单元测试中运行，不连接 ACP、读取账号、写入 `QuotaEvidence` 或影响准入。

TokenTracker 是独立设计对照，不是 Grok 上游接口合同。对照固定在 [`xiufengsun/TokenTracker` commit `24614b13d37c0f602808fbd88f0aabba743c96a0`](https://github.com/xiufengsun/TokenTracker/commit/24614b13d37c0f602808fbd88f0aabba743c96a0)，包括其 [`src/lib/grok-limits.js`](https://github.com/xiufengsun/TokenTracker/blob/24614b13d37c0f602808fbd88f0aabba743c96a0/src/lib/grok-limits.js)；该版本的 [`LICENSE`](https://raw.githubusercontent.com/xiufengsun/TokenTracker/24614b13d37c0f602808fbd88f0aabba743c96a0/LICENSE) 是 MIT，版权属 xiufengsun，2026。本文只参考了解析设计检查点：优先使用 `currentPeriod`、保留弃用字段，并且只有在分子和分母完整时才计算旧字段百分比；没有复制 TokenTracker 代码。它的凭据解析、认证刷新与重试、私有 billing HTTP 请求没有用于本实现，不构成已验证的 ACP 合同。

## 解除阻塞条件

后续若要验证具体 helper，固定其二进制、版本与隔离目录后，可选择核实可验证的官方源码/发布映射，或在单独授权且明确初始化副作用边界后进行有界兼容性验证；不要求无限期寻找 1.0.44 的完整源码。兼容性验证只说明实测协议行为，不证明来源、身份或费用约束。真人 OAuth、账号/额度读取与模型调用仍需分别授权。缺少可靠的身份、资格、额度和整次调用费用约束时继续显示 Unknown 并保持生成关闭。不得以 CLI 的 session token/cost、ACP prompt、网站手工读数或调用前额度快照推断该请求不会使用额外额度。
