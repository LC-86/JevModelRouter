# Grok 订阅文本生成（Issue #20）

本票实现 Grok CLI/ACP 的受限文本子集，并通过本地 ACP 替身验证适配行为。真实 CLI/OAuth、真实模型调用、额度与生产入口尚未验证；生产生成仍 fail-closed，需由 #26 完成人工核验后另行启用。

## 支持范围

- Chat Completions、Responses、Messages：单条非空用户文本消息；非流式 JSON 和增量 SSE。
- 输出 token 上限仅在 ACP 会话公开精确的 `max_tokens` 选项、接受所请求的整数值并回读确认后派发。Messages 必须提供 `max_tokens`。
- ACP `end_turn` 正常完成；`max_tokens` 作为受限完成返回：Chat `length`、Responses `incomplete/max_output_tokens`、Messages `max_tokens`。
- 每次请求新建辅助进程、ACP 会话及权限为 `0700` 的临时工作目录。账号使用自己的 Grok home；同一账号的生成串行，退出/世代变化会取消该世代请求。
- ACP 初始化、认证、会话创建及参数配置 RPC 各限时 15 秒；生成轮次限时 120 秒，文本输出上限 16 MiB。取消通知 ACP 并回收本请求进程；订阅派发不做内部重试。

不支持的字段、工具调用、图片及其他非文本输入、系统/助手历史和 Gemini 协议均在上游派发前拒绝。ACP 未给出可确认的终态、产生权限请求或在部分输出后失败时，返回失败终态，不拼接其他模型输出。

## Agent 验证

| 命令 | 结果 |
| --- | --- |
| `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib` | 通过（生产配置；生成派发仍 fail-closed） |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib grok::generation::tests -- --nocapture` | 7 passed；覆盖三协议解析/转换、流式、token-limit 终态、部分失败不 fallback、隔离目录、取消、轮次超时及默认适配器拒绝测试旁路 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --quiet` | 主机 loopback/子进程权限下 423 passed、0 failed；默认沙箱权限曾有 25 项因 `Operation not permitted` 失败，提权后全量通过 |
| `TAURI_DEV_HOST=127.0.0.1 pnpm test` | 13 个文件、89 项通过；未设置该环境变量时 Vitest 启动因沙箱 `localhost` DNS 解析失败 |
| `pnpm build` | 通过（TypeScript 与 Vite；有既有分包大小提示） |
| `pnpm release:check` | 通过 |

`pnpm test:isolated` 未运行：原生桌面 watchdog 曾超时，且本票无需扩大修复该环境问题。真实 Grok CLI、OAuth、额度与模型请求都未执行，亦未消耗 credits。

ACP 的终态及会话配置选项遵循 [Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn) 与 [Session Config Options](https://agentclientprotocol.com/protocol/v1/session-config-options)；这不代表 Grok CLI 的实际 ACP 行为已由真人核验。
