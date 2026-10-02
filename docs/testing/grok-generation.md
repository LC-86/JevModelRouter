# Grok 订阅生成与客户端工具往返（Issue #20 / #22）

本目录包含 Grok CLI/ACP 的受限文本适配与本地 ACP 替身测试。真实 CLI/OAuth、账号、模型资格、额度和整次调用的额外消费边界没有验证；生产生成仍 fail-closed。即使人工确认某个 CLI 行为，也必须先有可引用的身份、资格、额度与费用上限契约，才能考虑另行打开生产入口。

## 支持范围

- Chat Completions、Responses、Messages：非流式 JSON 和增量 SSE；纯文本生成沿用 #20 的受限输入子集。
- 客户端函数工具只接受受限 JSON Schema 子集：对象必须显式 `additionalProperties: false`，嵌套深度、字段数、描述、参数和调用数均有上限；工具选择只支持 `auto`/`none`，不支持强制指定工具。
- 仅接受与本次请求中声明的工具名和 schema 匹配、状态仍为 ACP `pending` 的工具调用。收到后立即取消 ACP prompt，并等待 `cancelled` 终态；ACP 报告执行中、已完成或其他工具/权限行为时明确失败。辅助进程继续以 `--tools ""` 启动，ACP 会话不提供 MCP 服务，工具执行权只交给下游 API 客户端。
- Chat、Responses、Messages 的调用 ID 与多工具结果一一关联。续轮必须绑定同一服务商连接世代、模型、协议、schema、输入历史和整批 call ID；完整结果（包含客户端标记失败的结果）才交回模型。部分结果会撤销整批 pending handoff，取消、断线或旧世代结果不会重放工具。
- 输出 token 上限仅在 ACP 会话公开精确的 `max_tokens` 选项、接受所请求的整数值并回读确认后派发。Messages 必须提供 `max_tokens`。
- ACP `end_turn` 正常完成；`max_tokens` 作为受限完成返回：Chat `length`、Responses `incomplete/max_output_tokens`、Messages `max_tokens`。
- 每次请求新建辅助进程、ACP 会话及权限为 `0700` 的临时工作目录。账号使用自己的 Grok home；同一账号的生成串行，退出/世代变化会取消该世代请求。
- ACP 初始化、认证、会话创建及参数配置 RPC 的 stdin 写入/flush 和响应读取共用 15 秒 deadline；生成轮次的 120 秒 deadline 从 prompt 写入前开始，覆盖写入/flush 与响应读取。取消通知最多写入 250 毫秒，失败时终止并回收本请求进程；订阅派发不做内部重试。

不支持的字段、不可约束的 schema、图片及其他非文本输入、系统指令和 ACP 无法保留的历史形式均在上游派发前拒绝。ACP 未给出可确认的终态、产生权限请求或在部分输出后失败时，返回失败终态，不拼接其他模型输出。

## 上游边界

Issue #22 的客户端工具闭环由受控 ACP 替身覆盖；它不证明真实 Grok CLI 会按同一方式报告 `pending` 工具调用。ACP 是代理/生成接口；`session/prompt` 会提交生成请求，可能消耗额度，不能用于只读身份、目录或额度探测。生产入口继续 fail-closed，替身不能开启生产入口。ACP 的 `tool_call` 状态和取消终态遵循 [Tool Calls](https://agentclientprotocol.com/protocol/v1/tool-calls) 与 [Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)。

## Agent 验证

| 命令 | 结果 |
| --- | --- |
| `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib` | 通过；生产配置下生成入口仍 fail-closed |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib grok::generation::tests -- --nocapture` | 18 passed；包括三协议双工具往返、完整/部分结果关联、取消、旧世代、无关请求和 ACP 反例 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --quiet` | 主机允许本地 loopback 与子进程操作时 447 passed、0 failed；受限沙箱首跑 421 passed、26 项因 `Operation not permitted` 失败，提权复跑通过 |
| `pnpm test` | 13 个文件、89 项通过；沙箱首跑无法解析 `localhost`，本地测试环境复跑通过 |
| `pnpm build` | 通过 TypeScript 与 Vite；保留既有 Tauri API 动态/静态导入及分包大小提示 |
| `pnpm release:check` | 通过 |

上述命令结果是本文件既有的历史记录，不构成本次 Issue #26 修复分支的测试结果。当前分支结果见 [人工验收入口](grok-hand-run.md) 和提交记录。真实 Grok CLI、OAuth、额度、credits 与模型请求均未执行。

ACP 的终态及会话配置选项遵循 [Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn) 与 [Session Config Options](https://agentclientprotocol.com/protocol/v1/session-config-options)；这不代表 Grok CLI 的实际 ACP 行为已由真人核验。
