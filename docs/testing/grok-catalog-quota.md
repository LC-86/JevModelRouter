# Grok 目录与额度适配状态（Issue #16；设计契约已撤销）

**当前生产行为：账号身份、模型目录、订阅池和额外用量均为 Unknown。** 本文件旧版定义的 `account --json`、`models --json`、`usage --json` 与 `account/catalog/quota` JSON 行事件，是未获上游支持证据的应用侧假设，已从生产适配路径撤下。不得手动尝试这些命令，也不要将旧版 JSON schema 当作 Grok CLI 契约。

## 当前实现

- 生产 `GrokCliAuth::begin` 在准备 helper home 或创建进程前拒绝登录，错误码为 `grok_auth_unverified`；生产 helper 状态报告 `login_supported=false`。
- 生产 Grok 只读状态不创建 CLI 子进程。身份状态不可核实时返回明确错误；模型目录及额度证据保持 `Unknown`，缺少的身份、目录、窗口、许可和额外用量字段以 `missing_fields` 记录。
- UI 未验证身份显示 `Unknown`。未知订阅许可不通过准入；未知整次调用额外消费限制时拦截派发。Unknown 不代表余额为 0、无额度或无额外费用。
- 本地 Rust fake-helper 单测仍可用显式测试注入走旧映射 fixture，用于检查防回归和 UI 呈现。这只验证 AutoJev 的本地映射逻辑，不代表生产接口或账号行为成立。

## 接口核对结果

本机 `@xai-official/grok` 1.0.44 的只读帮助显示：`account` 不可用；`models --help` 没有 `--json`；`usage` 接受本地 session ID，统计 session token/cost，不是订阅额度或 credits API。没有启动登录、读取账号/session、运行模型目录/额度请求或 ACP。

官方 CLI 文档只把 `models` 描述为可用模型列表；它没有确认本机 1.0.44 对下列候选接口的支持。官方 [xai-org/grok-build 公开 commit `72a61251fcffb464bcc687aeb5a998e5a98ec0c9`](https://github.com/xai-org/grok-build/commit/72a61251fcffb464bcc687aeb5a998e5a98ec0c9) 的提交信息标注 `Source-Revision: a549186d9d39311f2d3ee4208db62af8c65aa476`；前者是公开 GitHub commit SHA，后者是其来源版本标识，不是公开 commit SHA。该固定快照中的 [`xai-grok-shell/Cargo.toml`](https://github.com/xai-org/grok-build/blob/72a61251fcffb464bcc687aeb5a998e5a98ec0c9/crates/codegen/xai-grok-shell/Cargo.toml) 与 [`CHANGELOG.md`](https://github.com/xai-org/grok-build/blob/72a61251fcffb464bcc687aeb5a998e5a98ec0c9/crates/codegen/xai-grok-shell/CHANGELOG.md) 都标为 `1.0.16`（2026-09-01）。这份快照包含 `x.ai/auth/info`、`x.ai/auth/check_subscription`、`x.ai/models/list`、`x.ai/billing` 与 `x.ai/auto-topup-rule` 候选 ACP 方法；不能把快照版本 `1.0.16`、本机 CLI `1.0.44` 与官网当前版本混为一谈。它不证明本机 CLI 1.0.44（本机源码标识 `5b807183dd79`）实现了同一契约。

这些候选方法的语义也不足以放开生产准入：`auth/info` 的 `current_or_expired` 状态不证明登录当前有效；`auth/check_subscription` 会刷新 JWT，是有状态网络操作，本只读验收不得调用；`models/list` 不证明模型具备订阅内资格。`billing` 与 `auto-topup-rule` 可读用量、周期、预付余额、按需 cap/已用量与远端自动充值设置，但 `on_demand_enabled` 是远端设置状态，不是阻止消费的执行保证；检查到的 GET 方法也没有提供“整次调用不扣额外 credits”的保证。官方 [Usage & limits FAQ](https://docs.x.ai/grok/faq#usage--limits) 说明订阅周额度用尽后会使用 Extra Usage credits；关闭自动充值不等于已有余额不会被消费。因此，本地版本是否支持这些方法、身份有效性、模型资格和禁止额外消费的保证仍为 **Unknown**。本机 `grok account` 不可用、`models --help` 无 `--json`、`usage` 读取本地 session token/cost 的事实不变。CLI headless/ACP 的 `session/prompt` 会生成内容，不能用于只读探测。

## 验收

运行 `pnpm test:grok-contract` 只验证本地 fake-helper 合同拒绝、Unknown 展示和前端状态文本。它不运行 Grok CLI，不证明真实身份、目录、额度、Extra Usage 权限或费用上限。安全人工验收入口和当前阻塞项见 [grok-hand-run.md](grok-hand-run.md)；结果记录模板见 [grok-hand-run-result-template.md](grok-hand-run-result-template.md)。

## 解除阻塞条件

在任何真人 OAuth 或模型调用前，先为固定版本取得可引用的上游接口契约，覆盖机器可读身份、目录及调用资格、订阅额度与额外用量许可，以及能约束整次请求费用的机制。缺少任一关键事实时继续显示 Unknown 并保持生成关闭。不得以 CLI 的 session token/cost、ACP prompt、网站手工读数或一个请求前的额度快照推断该请求不会使用额外额度。
