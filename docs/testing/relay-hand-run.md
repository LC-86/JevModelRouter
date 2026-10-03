# R6 → R7：逐来源 HAND_RUN

唯一规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，交付 [#57](https://github.com/LC-86/JevModelRouter/issues/57)，**所有真实结果只回填 [#58](https://github.com/LC-86/JevModelRouter/issues/58)**。当前计划未批准真实动作；真实凭据配置、登录、账号额度查询、模型调用均未执行，次数 **0**。本表不是登录或消费许可。

## 当前来源范围

每个实际账号、套餐、endpoint、连接实例/世代、模型和协议单独复制一份[计划及结果模板](relay-hand-run-result.md)。同名来源、两个账号或两个套餐不得共用一条验收。未知费用写 **Unknown**，不得写 0。

| 来源 | 账号 / 套餐 | 版本与路径范围 | 模型 / 协议 / 工具轮次 | 可能费用 | 当前允许步骤 / 状态 |
| --- | --- | --- | --- | --- | --- |
| CPA Codex 订阅 | Unknown / Unknown | Jev 完整 SHA + artifact manifest；CPA `r1-e2bff010` / v8，独立专用 profile；DSH `0.1.7-rc.1` / pi-ai `0.85.1` 接口 pin | 上游 ID 与稳定 UUID 待填；Chat Completions 待真人验证；工具 0 | Unknown；整次调用仅订阅内权益保证未成立 | 仅阅读本包；凭据/登录/额度/生成预算均 0；未测、禁用 |
| CPA Grok 订阅 | Unknown / Unknown | 相同 Jev/CPA pin；本版本普通 OAuth 固定账号与 Grok 接入契约未建立 | Unknown / 未验证；工具 0 | Unknown；Extra Usage 保证未成立 | 仅阅读；真实服务接入不支持/未验证，不尝试登录；预算 0 |
| Coding Plan | Unknown / Unknown，必须写具体产品 | Jev manifest；实际 Key 与套餐专用 endpoint 待填；若用 CPA 则记录其 pin、单凭据 namespace | 上游 ID + UUID 待填；Chat Completions 待验；工具 0 | Unknown；不能改用同品牌普通 API endpoint | 仅阅读；配置/查询/生成预算 0；未测、禁用 |
| 官方 API | Unknown / Unknown | Jev manifest；具体官方 endpoint、API 版本待填；使用既有 API 派发路径 | 上游 ID + UUID 待填；Chat Completions 待验；工具 0 | Unknown；须具体费用上限及依据 | 仅阅读；配置/生成预算 0；未测 |
| OpenRouter 第三方 API | Unknown / Unknown | Jev manifest；实际入口、账户及计费范围待填；生成来源与决策供应商分开 | 实际上游 ID + UUID 待填；Chat Completions 待验；工具 0 | Unknown；须具体费用上限及依据 | 仅阅读；配置/生成预算 0；未测 |
| ZenMux 第三方 API | Unknown / Unknown | Jev manifest；实际入口、账户及计费范围待填；生成来源与决策供应商分开 | 实际上游 ID + UUID 待填；Chat Completions 待验；工具 0 | Unknown；须具体费用上限及依据 | 仅阅读；配置/生成预算 0；未测 |

运行前 Leo 必须填写并明确批准该**单一来源**的完整计划：Mac 架构/系统、Jev/CPA source/artifact/checksum/许可、DSH 版本及实际 composition、独立目录、非秘密账号/套餐身份依据、连接实例/世代、模型资格/协议能力、适用额度及整次调用费用约束、允许操作和有限预算。缺一项维持未测/禁用。API Key、目录、usage、成功文本均不代替费用或权益许可。

## 条件成立后才可使用的有限步骤

1. 核对 [Mac 开发产物](mac-service-r6.md) manifest 与服务状态。隔离回环产物只含虚构来源，不接真实账号；生产 CPA 授权当前未配置。#58 解阻必须有实际专用服务与账号定向契约，不能把隔离开关或替身改成真实入口。
2. 逐项核实身份、套餐、资格、协议、额度/费用。只读步骤也需在计划中分别批准和计数。服务启动/恢复、保存来源和模型选择本身不得触发登录、额度或生成。
3. 官方 API/第三方来源使用既有来源表单，填准确 endpoint、计费身份和模型；Coding Plan 只填套餐专用 endpoint。选择模型池行的稳定 `autojev/model/<UUID>`，在独立虚构 DSH composition 中使用 R5 [配置片段](fixtures/dsh-fictional.patch.yml)，`maxRetries: 0`。不写日常 DSH 配置、不启用自动测速/智能路由。
4. 来源、模型启用是用户配置，不能使未知身份/世代、失去绑定、资格/协议未验证或订阅费用依据缺失变成有效。CPA 订阅生产入口仍不可配置，等待 #58 的适用证据和受控接入；不修改数据库或源码来解除门禁。
5. 恢复服务只恢复自有进程及已保存 endpoint，不授予真实能力。换号/套餐变化后旧证据和引用失效；重新验证并明确绑定后才讨论受控启用。取消选择不等于撤销；停用后旧 UUID 直调仍须拒绝。

当前上述真实执行步骤均为**未允许**。将来批准范围只作用于该来源/账号/套餐/世代/模型/协议/artifact，不自动扩到其它行。

## 单来源候选调用矩阵

批准前总调用预算 **0**；下面最多 7 次网关请求的候选计划须由 Leo 逐项接受或删减，不能在失败后把未用预算移给重试或其它来源。每次输入（完整消息/工具 schema/结果）最多 2048 UTF-8 字节，输出 `max_tokens: 64`，收集超时 15 秒。取消只证明本地流释放，不承诺上游停止计费。

| 步骤 | 虚构输入 / 客户端工具 | 最大网关请求数 | 辅助/续答轮次 | 当前执行 |
| --- | --- | --- | --- | --- |
| 文本 | `Reply exactly: HAND_RUN_OK` | 1 | 0 | 未测 |
| 多轮 | 同一固定目标：`Remember fictional color blue`，再 `What fictional color?`；保留完整历史 | 2 | 第二问计新请求 1 次 | 未测 |
| SSE 终态 | `Reply exactly: HAND_RUN_STREAM_OK` | 1 | 0；必须看到唯一完成/失败终态 | 未测 |
| 取消 | `Write a fictional list`；首个增量后客户端取消 | 1 | 0；终态不明立即停 | 未测 |
| 客户端工具 | 仅 `fixture_lookup({"item":"fixture-1"})`；客户端返回固定 `{"value":"fixture-value"}`；关联同一 call ID | 最多 2 | 工具最多 1 轮；结果续传另计 1 请求 | 未测 |

工具只在独立客户端内构造固定结果，不接文件、命令、浏览或外网；Jev/CPA 不代执行。需要其它协议时另写单协议计划，未验证能力不得公布支持。核对同名其它账号/套餐/付费来源接收数为 0，真实上游身份须有直接证据，响应 `model` 字符串不能证明来源。固定失败测试必须事前单独批准，第一次预期拒绝/失败后即停止整份计划。

## 停止与回报

任一失败、准入拒绝、身份/世代/模型/协议不符或 Unknown、版本或隔离不符、费用/额度依据缺失、取消/终态不明、意外工具、预算越界或未知派发，立即停止剩余步骤。**不重试、不扩量、不换账号/模型/协议/来源，不切付费 API，不为补证登录或刷新。** 失败、取消与已开始派发均记实际次数；本地拒绝同时记录网关尝试和上游派发 0 的依据。无上游计数证据写 Unknown，不把本地请求数当真实消费次数。

按[结果模板](relay-hand-run-result.md)回填 #58：每项通过/失败/未测/不支持、实际网关/上游/工具/辅助次数、用量/费用来源和时间、脱敏证据、停止原因。Agent 只分析已提供的脱敏结果，不补跑真人步骤，不关闭 #57/#58。

## #25/#26 资产的适用范围

- [Codex HAND_RUN](codex-hand-run.md)、[登录交接](codex-login-handoff.md)：复用非秘密账号别名、独立目录、有限预算、终态/失败即停与费用 Unknown 记录方法；CLI helper 版本、登录/额度接口和旧 CLI 文本证据保持原作用域，不移作 CPA 成功。
- [Grok HAND_RUN](grok-hand-run.md)、[结果模板](grok-hand-run-result-template.md)：复用输入/本地假工具、派发/辅助轮次分别计数、Extra Usage 未知拒绝和逐字段证据；旧 ACP/CLI 检查不证明 CPA Grok 支持、真实授权或费用限制。
- [R5 DSH 接口与回放](dsh-r5.md)：复用固定接口 pin、虚构配置、UUID 选择、JSON/SSE 工具 ID/结果与取消检查。实际 DSH composition/选择器和真实来源均仍未验。

#25/#26 仍保持真人未完成事实。仅在 #58 相应真实必需项通过后，由父线程将该来源的结果链接回旧票；一项来源通过不覆盖其它来源。
