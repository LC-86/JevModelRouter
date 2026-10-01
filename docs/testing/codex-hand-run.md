# Codex #25 HAND_RUN：真人登录与受控生成核对

**状态：Agent 准备已完成；真实账号、权益和生成结果尚未验证。** 本清单交给 Leo 手动执行并回填。Agent 只用隔离替身验收；不得以替身结果代替真实证据，也不得据此关闭 [#25](https://github.com/LC-86/JevModelRouter/issues/25) 或父票 [#10](https://github.com/LC-86/JevModelRouter/issues/10)。

## 0. 这版代码的当前拦截项

本分支的开关只表示“我已确认一份有限的手测计划”，**不是生成许可**。每条请求仍须同时通过 #12/#18/#17 的身份与世代、当前只读证据、账号目录资格、模型/协议能力、订阅额度、额外 credits 禁用等既有检查。开启开关不会补写或提升任何证据。

开始任何真实生成前，先在 `Providers` 查看 Codex 行：

- 目录发现项当前仍是 `eligible=false`，不能调用；不得在数据库、UI、测试夹具或报告中手填 `eligible=true`。
- 能力必须按**具体模型 × 协议**真实核验为 `Verified`。未验证或 unsupported 就停止；不得手动改成 `Verified`。
- `credits.permission` 当前映射缺口会留下 `Unknown`；额度门禁会拒绝。额度或 credits 为 Unknown、Denied、Failed、Stale，或无法确认额外用量已关闭时，**不点生成测试按钮**。
- 本地并没有硬性输出 token 上限。Codex 请求白名单拒绝 `max_tokens` / `max_output_tokens`；提示词里的“请简短回答”不是硬上限。取消也不能证明上游已停止计量。测速的 64 KiB 只是本地收集限制。
- `Providers` 中订阅测试只覆盖 Chat Completions 文本；`Models` 的手动测速每模型固定最多发 3 个流式请求。

因此，按当前已知证据，这票可完成身份和只读状态手测；真实生成矩阵应先记为 **未测／准入阻止**。待资格、能力与 credits 证据通过受审变更真实落地后，再继续第 3 节。此文件不授权绕过上述拦截。

## 1. 登录与只读状态（不生成）

1. 启动本票对应桌面版。点侧栏底部 `Settings` → `About`，记录 AutoJev `v...` 和构建类型；在本票报告中另记录交付时给出的 Git commit。不要把令牌、授权码、完整邮箱或 `~/.codex` 内容贴进报告。
2. 点侧栏 `Providers`，找到 `Codex subscription` 行，记录行内非秘密账号别名、`generation=...` 和 helper 版本。尚未登录时，点该行 `Sign in`，只在官方授权页完成登录；核对授权前后账号确为预期账号。
3. 如果登录页主机不是 `auth.openai.com` 或 `chatgpt.com`，先停止并记下主机；不要输入账号密码或批准未知站点。授权完成后，确认行内显示身份且状态是 `connected`。界面若不能证明账号身份，停止。
4. 点同一行的 `Refresh read-only status`。这一步只读账号、模型目录与额度，不发生成请求。记录模型 id、目录来源/时间、每个额度桶的许可/窗口/时间、credits 原始显示与许可状态。截图只保存在本机脱敏目录，遮盖完整账号与任何 URL 查询参数。
5. 若需检查退出/换号，先确保没有在途 Agent 请求；通过同一行的 `Sign out` 或 `Switch account` 操作，记录退出后身份、世代、目录与额度清理结果。重新登录时重复第 2–4 步。未获 Leo 确认前不要尝试额外 credits、reset、自动充值、付费设置或账号恢复。

**截至本分支交付，以上真实登录/只读项都应标为“未测”**；Agent 没有登录，也没有访问真实额度。

## 2. 每次真实生成前：由 Leo 填并复核

开始前填写一份新的运行计划；空项、未知费用或无法确认只订阅内消耗时，不开启真实生成开关。

| 运行计划字段 | Leo 填写 |
| --- | --- |
| 日期、AutoJev 版本/构建 commit | |
| Provider / 固定 helper 版本 | |
| 非秘密账号别名 / 当前连接世代 | |
| 精确模型 id / 精确协议（Chat、Responses、Messages） | |
| 虚构输入（不含客户、密钥、私有代码或个人数据） | |
| 工具范围（`None`，或仅描述一个由客户端执行、无文件/命令/网络能力的安全工具） | |
| 允许的 AutoJev 请求总数（1–15） | |
| 计数拆分（文本、流式、取消、每个工具结果后续请求、额外轮次） | |
| 输出边界（明确本地没有硬 token 上限；若硬上限是前提，停止） | |
| 可能费用（确认为订阅内；不确定写 `未知`，不得写 `$0`） | |
| 允许步骤 / 禁止步骤 / 停止联系人 | |
| Leo 确认人、时间 | |

三种协议完整排练的**计划基线**为每个模型 15 个 AutoJev 请求：Chat、Responses、Messages 各 1 个文本 + 1 个流式 + 1 个工具调用 + 1 个工具结果后续请求，共 12 个；`Models` 的一次手动 Speed test 最多 3 个流式请求，用来覆盖停止尝试，共 15 个。每个额外工具结果/助手轮次另加 1；失败和取消也占一个已用尝试；不重试。实际 UI 是否支持这些操作见第 3 节。除这 3 个已列入计划的请求外，手动测速不得再点。

检查 `quota=available` 且当前账号订阅内许可明确；确认 credits 许可明确为 `denied`（额外用量关闭）。`Unknown` 不等于 0 费用，也不满足确认。若有任何身份、世代、模型资格、协议能力、额度或 credits 项不匹配，停止而不是改证据。

## 3. 有条件的按钮级生成步骤

只有第 1 节证据真实、当前模型和协议的既有准入均已通过、Leo 已填完第 2 节并确认可能费用后，才执行此节。每次请求都必须使用计划中的同一账号/世代和模型；运行中不切换来源。

1. 在 `Providers` 页底部的 `Real Codex generation` 区域输入本计划的最大请求数（`1–15`），点 `Enable real Codex generation for this connection`，再确认弹窗。弹窗只确认计划，不发送请求。界面会显示 `x of n confirmed AutoJev requests remain`。关闭再开、同世代内失败的登录尝试或重命名不会恢复已用预算；预算耗尽后该世代不能再次启用。成功退出、换号或重启后，必须针对当前连接重新确认。
2. **单次文本请求：** `Providers` → Codex 行 `Test`。这项订阅测试只使用 Chat Completions 文本；每次点击记 1 个计划请求。若按钮因未选择已发现模型而不可用，或准入拒绝，记下原样拒绝并停止。
3. **指定协议与流式：** `Debug` → `Model / route` 选已确认的 Codex 目标 → `API type` 选计划协议 → 输入计划中的虚构输入 → 如需流式，点 `Add parameter`，填 `stream` / `true` → `Send test request`。每次发送计 1。记录助手结果与终态；HTTP 响应中的 `model` 是本地请求目标，`id` 由网关生成，二者都不能独立证明实际上游模型或账号。只有另有独立适配器证据时才填写已核实的上游身份，否则记录 `Unknown`。Debug 目前没有单请求 Stop 按钮，不能把关闭窗口当成可靠取消。
4. **取消尝试：** 当前可见的取消入口是 `Models` 页的 `Speed test` → 只选计划中的一个 Codex 模型 → `Speed test`；正在运行时点 `Stop speed tests`。此测速最多发 3 个流式请求，计划必须预留 3 次；在界面显示完成/停止请求数后如实记录。它只提供本地响应收集限制，不能证明上游停止计量。若不接受可能费用/无法约定停止边界，标记“未测”，不要尝试。
5. **客户端工具往返：** `Debug` 不会执行客户端工具。只有在 `Agents`/客户端中能选用一个独立、可丢弃的配置，且工具是 Leo 确认的客户端本地安全工具时才排练；不得用日常 Agent、文件/命令/浏览/网络工具。首个工具调用请求计 1，每个包含工具结果的后续请求各计 1。若没有这样的现成隔离客户端，标记“未测”，不临时改主 Agent 配置。
6. 预算减至 0、界面计数与计划不一致、或任何准入拒绝/响应错误时，停止后续点击。不得再点 `Test`、`Debug Send`、`Speed test`，不得更换模型/协议/账号重试。

当前已知 eligibility/capability/credits 门禁使真实生成步骤无法完成；这不是让 Leo 自行取消或修改门禁的提示。隔离替身验证只覆盖 UI 开关与门禁拒绝，未验证真实调用。

## 4. 观察字段与停止条件

每次请求单独记一行，记录：provider；固定 helper 版本；非秘密账号别名及连接世代；计划模型 id；协议；请求序号/总数；工具或 helper 后续轮次；额度来源与读取时间；credits 权限；HTTP 返回的本地请求 model 与网关 response id；若独立上游证据存在，另记其来源和结果，否则写 `Unknown`；最终状态（completed / failed / cancelled / interrupted / 未开始）与可脱敏错误；本机脱敏证据路径；已知 usage/cost 或 `Unknown`。

出现任一项立即停止，不重试、不 failover、不切账单来源：身份/世代/模型/协议与计划不符；计数越界；额度或 credits 异常/未知；无法确认仅用订阅额度；协议或模型准入拒绝；响应身份错、错误或终态缺失；取消状态无法确认；费用超出运行前同意范围。

## 5. Leo 回填格式

```text
状态：通过 / 失败 / 未测
AutoJev 版本 + commit / Codex helper 版本：
账号别名 + generation（不得填完整邮箱）：
模型 id + 协议：
计划请求数 / 实际请求数 / 工具与后续轮次：
quota 来源 + observed_at / quota permission / credits permission：
HTTP 请求 model / 网关 response id / 独立上游 identity 来源与结果（无则 `Unknown`）+ terminal/error：
已知 usage/cost（未知就写 Unknown）：
脱敏证据目录：
停止原因、后续项：
```

Leo 回填真实结果；Agent 不代填、不补跑。完成手测只更新 #25/#10 的人工证据状态，不由 Agent 关闭 issue。

## 依据

- 父规格 [#10](https://github.com/LC-86/JevModelRouter/issues/10)；人工验收 [#25](https://github.com/LC-86/JevModelRouter/issues/25)。
- Codex App Server 当前官方协议说明：[App Server](https://learn.chatgpt.com/docs/app-server)、[`account/rateLimits/read` 等账号协议](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/account.rs)、[turn/interrupt 与 turn 事件协议](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/turn.rs)。本清单不调用会消费 reset 的接口。
- 既有本地证据：[Codex 登录交接](codex-login-handoff.md)、[Codex 目录与额度](codex-catalog-quota.md)。
