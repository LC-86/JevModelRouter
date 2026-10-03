# R5：DSH 固定目标与客户端工具证据

唯一规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，实施票 [#56](https://github.com/LC-86/JevModelRouter/issues/56)，基线 `a748034e1de30f0d05c4c96acd01bdb3475e5791`。本轮只使用虚构输入、凭据和回环上游；真实模型调用、登录、账户额度查询均为 **0**。实际 DSH 与真实来源由 [#58](https://github.com/LC-86/JevModelRouter/issues/58) 承接。

English: this delivery pins installed DSH interfaces and replays their HTTP shape through an isolated native Jev application and checksum-pinned CPA. It does not run DSH or establish real subscription eligibility, capabilities, or billing guarantees.

## 已读取的接口

只读已安装的发布代码，不执行 DSH、不读取用户配置或凭据库。DSH CLI 及相关包固定 `0.1.7-rc.1`，其 pi-ai 为 `0.85.1`；[11 个发布文件的 SHA-256](dsh-interface.json) 可用于升级后重验。包库存中存在此能力，不表示用户当前实际挂载了这些插件。

| 发布包 / 文件 | 可配置契约与证据位置 |
| --- | --- |
| `@deepseek-ai/dsh-llm-pi-ai/lib/index.js` | `Config.providers` 支持 `api`、`baseURL`、`apiKeyEnv`、显式 `models[].id`、`compat` 和 `retryPolicy`；`PiAdapter.streamWithSnapshot` 捕获固定模型并传入取消信号 |
| `@earendil-works/pi-ai/dist/api/openai-completions.js` | `api: openai-completions` 调用 `POST {baseURL}/chat/completions`；携带 `model.id`、完整消息历史、客户端函数 schema；保留 `tool_calls[].id`、`tool_call_id` 和流式参数/终态 |
| `@deepseek-ai/dsh-agent-default-model/lib/index.js` | 配置 `{provider, model, reasoningEffort?}` 选择默认模型 |
| `@deepseek-ai/dsh-api-session-controller/lib/index.js` | 已挂载控制器的 `selectModel({sessionId, provider, model})` 调用 `selectForNextRequest`，可按现有接口保存默认选择；这里没有猜测或新建 HTTP 选择 URL |
| `@deepseek-ai/dsh-agent/lib/index.js` | `installModelSelection` 在下一次 prompt 组装时捕获 provider/model，已有生成使用先前快照 |

`GET {baseURL}/models` 是发现入口；目录不是资格证据。兼容入口名称 `openai-completions` 在这里表示 Chat Completions，不能从名称推断成旧文本 completions。现有 Jev Agent adapter 未被声明为支持 DSH。

## 可复制的虚构配置与选择

[配置片段](fixtures/dsh-fictional.patch.yml) 供独立虚构 DSH composition 复用已有插件。它不是完整启动配置，也不应写入日常 profile。本轮没有启动这份配置。复制时把 `19527` 和两条示例 UUID 替换为隔离 Jev 实际监听端口及模型池展示的稳定调用 ID；为 `JEV_FICTIONAL_KEY` 提供任意虚构字符串。该 key 是本地客户端占位符，不是 CPA 管理 key 或上游真实凭据。

- Jev 本地 `baseURL` 为 `http://127.0.0.1:<isolated-port>/v1`，模型入口为 `/v1/chat/completions`。
- 默认 `{provider: jev-local, model: autojev/model/<A-UUID>}` 固定来源 A；同名裸 ID、显示名和 `autojev/auto` 不承担固定来源选择。
- 已挂载的会话控制器用 `selectModel({sessionId, provider: 'jev-local', model: 'autojev/model/<B-UUID>'})` 设置下一请求。不要把配置片段当作控制器挂载证明；实际调用方式由 DSH composition 决定，留待 #58。
- 配置显式 `maxRetries: 0`，因为此 DSH 版本 normal 默认会重试 5 次；`supportsReasoningEffort: false`，不发送未验证的推理语义。测试的文本输出上限为 64。

虚构文本请求可复制为：

```json
{"model":"autojev/model/11111111-1111-4111-8111-111111111111","messages":[{"role":"user","content":"fictional question"}],"stream":true,"max_tokens":64}
```

工具由客户端执行：提交 `tools` → 收到 assistant `tool_calls` → 客户端提供同一 `tool_call_id` 的 `role: tool` 结果 → 将完整历史发回同一固定 UUID。测试只构造虚构结果，没有执行 shell、文件或其它工具。

## 证据分层与边界

| 层次 | 本轮观察 | 不能推出的结论 |
| --- | --- | --- |
| 发布代码只读 | 固定 DSH/适配器版本、endpoint、配置和选择快照语义 | 用户的实际 DSH composition 已挂载或运行 |
| 公开网关协议回放 | 临时 SQLite、DSH 形状请求、固定 UUID；未知资格、未连接、身份世代变化、失去绑定、停用均拒绝，文本/SSE/tools 的同名付费接收端为 0 | 真实账号或订阅允许生成 |
| 隔离 Mac 原生 UI | 公开表单保存四个虚构来源、稳定 UUID/实例/世代重启不变；模型池选择操作与实际增量流相交叠；Debug、手测和测速共用准入 | 实际 DSH UI 已完成手选 |
| 实际 CPA | 沿用 [R1 manifest](../../scripts/cpa-artifact.json) 的二进制校验和，普通模型 HTTP 经唯一 prefix 到虚构 A1/A2/B1/API 上游；实际接收记录验证凭据/模型/次数 | OAuth 普通生成已固定账号、真实能力或整次调用仅用订阅权益 |
| 真实 DSH / 来源 | **未运行**，由 #58 逐来源回填 | 当前已完成最终产品验收 |

实际 CPA 场景用 OpenAI-compatible API 凭据和唯一来源 prefix，不导入 OAuth 凭据。原生 API 表单的账号/套餐是虚构用户声明，能力仍显示未独立验证；订阅资格缺失始终拒绝。新增 Rust 回放在单独临时 DB 中构造非秘密订阅状态投影，未执行授权或认领真实凭据。

### 文本、工具与切换

接收端记录检查普通文本及完整多轮角色/内容。JSON 和 SSE 两种工具往返检查 schema、call ID、碎片 arguments、客户端结果和续答；未收到客户端结果时只观察到一次模型请求。两个并发工具会话分别固定 A1/A2，call ID 与结果互不串用，同名 B/付费来源为 0。

流式切换先读取 A1 的真实首个增量，再在原生模型池取消 A 的选择，并以相同 session ID 发出固定 B1 UUID 的后续请求；释放 A 后，旧流仍只有 A 内容并以 `finish_reason: stop` / `[DONE]` 完成。接收记录恰为 A1、B1。这补足 R4 在途流语义证据。**原生池选择是列表管理操作，后续请求的模型 UUID 由回放客户端改变；没有把它冒充成实际 DSH `selectModel` 调用。** DSH 下一请求选择快照有发布代码证据，实际运行仍待 #58。

### 失败、取消与准入

401、429、500、502、503 各只命中 A1 一次，A2/B/付费 fallback 为 0。部分输出后的错误保留已到达文本和明确错误事件，诊断为 `error`；因响应头已发送，HTTP 状态仍可能是 200，不能据此计成功。客户端 abort 在自有上游观察断开，Jev 诊断为 `cancelled`；这说明本地流释放，不承诺远端已停止计量。

Responses→Chat 的不支持 `background` 明确 422/零派发；新回归揭示 Chat→Messages 曾静默丢失 `reasoning_effort`，现跨协议明确 422/零派发。同协议保留现有透传能力，没有新建认证、协议转换内核或 DSH 插件框架。其它协议与多模态不由这次 Chat Completions 回放声明支持。

停用来源后固定入口、Debug、服务商测试与测速拒绝，接收端为 0。启动、保存和 snapshot 加 1.5 秒正常 idle 未生成；完整 Rust 回归另有既有 65 秒后台零自动生成/测速检查。保留现有请求 ID、请求 UUID、上游模型、来源和终态错误日志即可，没有增加统计或成本系统。

## 复现

先以显式安装文件和发布包库存路径生成只读 pin，版本变化会失败并要求重新核对。该命令不寻找用户 profile：

```sh
node scripts/inspect-dsh-interface.mjs /absolute/installed-dsh/lib/bin.js /absolute/pnpm/store/v11/links
```

在自有 clone、依赖及 target 中运行。原生驱动只启动独立临时 root、临时监听端口和自有进程；固定 CPA 先校验 SHA-256，继承环境仅 PATH/TMPDIR/语言变量，复用既有 loopback-only 隔离策略。需要本机已有的原生构建环境及自有窗口截图能力；不安装软件或新增权限。

```sh
TAURI_DEV_HOST=127.0.0.1 pnpm build
TAURI_DEV_HOST=127.0.0.1 pnpm test
pnpm release:check
CARGO_TARGET_DIR=/absolute/owned-target cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
CARGO_TARGET_DIR=/absolute/owned-target cargo build --locked --offline --manifest-path src-tauri/Cargo.toml --features isolation-check
node scripts/check-dsh-desktop.mjs /absolute/owned-target/debug/autojev /absolute/checksum-pinned-cpa --capture
```

驱动保留 `first.report.json`、`reload.report.json`、`receivers.json` 和自有 PID 的窗口截图。两份报告均须当前 run ID、`ok:true`、进程退出 0；UUID/来源身份一致，CPA 自有 PID 均已回收，才计通过。缺报告、锁屏、超时或旧 CLI/file-lock 波动不能计通过。截图失败单独记录，不更改权限；只回收本轮启动的进程。
