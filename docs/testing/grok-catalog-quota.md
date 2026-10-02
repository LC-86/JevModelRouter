# Grok 目录与额度适配状态（Issue #16；设计契约已撤销）

**当前生产行为：账号身份、模型目录、订阅池和额外用量均为 Unknown。** 本文件旧版定义的 `account --json`、`models --json`、`usage --json` 与 `account/catalog/quota` JSON 行事件，是未获上游支持证据的应用侧假设，已从生产适配路径撤下。不得手动尝试这些命令，也不要将旧版 JSON schema 当作 Grok CLI 契约。

## 当前实现

- 生产 `GrokCliAuth::begin` 在准备 helper home 或创建进程前拒绝登录，错误码为 `grok_auth_unverified`；生产 helper 状态报告 `login_supported=false`。
- 生产 Grok 只读状态不创建 CLI 子进程。身份状态不可核实时返回明确错误；模型目录及额度证据保持 `Unknown`，缺少的身份、目录、窗口、许可和额外用量字段以 `missing_fields` 记录。
- UI 未验证身份显示 `Unknown`。未知订阅许可不通过准入；未知整次调用额外消费限制时拦截派发。Unknown 不代表余额为 0、无额度或无额外费用。
- 本地 Rust fake-helper 单测仍可用显式测试注入走旧映射 fixture，用于检查防回归和 UI 呈现。这只验证 AutoJev 的本地映射逻辑，不代表生产接口或账号行为成立。

## 接口核对结果

本机 `@xai-official/grok` 1.0.44 的只读帮助显示：`account` 不可用；`models --help` 没有 `--json`；`usage` 接受本地 session ID，统计 session token/cost，不是订阅额度或 credits API。没有启动登录、读取账号/session、运行模型目录/额度请求或 ACP。

官方 [CLI reference](https://docs.x.ai/build/cli/reference) 将 `models` 描述为可用模型列表命令，没有定义本适配器所需的机器可读身份、订阅资格或额度 schema。官方 [Usage & limits FAQ](https://docs.x.ai/grok/faq#usage--limits) 指向产品设置里的用量页面，并描述额外购买/自动充值选项；这些资料不提供 AutoJev 可依赖的“整次调用不得产生额外消费”执行保证。[CLI headless/ACP](https://docs.x.ai/build/cli/headless-scripting) 的 `session/prompt` 会生成内容，有潜在额度消耗，不能用于只读探测。

## 验收

运行 `pnpm test:grok-contract` 只验证本地 fake-helper 合同拒绝、Unknown 展示和前端状态文本。它不运行 Grok CLI，不证明真实身份、目录、额度、Extra Usage 权限或费用上限。安全人工验收入口和当前阻塞项见 [grok-hand-run.md](grok-hand-run.md)；结果记录模板见 [grok-hand-run-result-template.md](grok-hand-run-result-template.md)。

## 解除阻塞条件

在任何真人 OAuth 或模型调用前，先为固定版本取得可引用的上游接口契约，覆盖机器可读身份、目录及调用资格、订阅额度与额外用量许可，以及能约束整次请求费用的机制。缺少任一关键事实时继续显示 Unknown 并保持生成关闭。不得以 CLI 的 session token/cost、ACP prompt、网站手工读数或一个请求前的额度快照推断该请求不会使用额外额度。
