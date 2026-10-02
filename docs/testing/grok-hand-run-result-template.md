# JevModelRouter #26：Grok 人工验收记录

**本提交的真实 Grok 验收结论：阻塞；本地 fake 契约回归：通过。** 填本文件不构成真实登录/生成授权。不要在接口、费用边界或隔离证据未知时尝试 AutoJev Grok 登录、读账号/额度或生成。

## 当前提交的只读前置核对

| 项目 | 观察结果 | 判定 |
| --- | --- | --- |
| 本机官方命名 CLI | `@xai-official/grok` 1.0.44；仅版本与帮助核对 | 已观察；不证明 AutoJev 实际会选择此路径或版本 |
| 命令列表 | 帮助列出 `login`、`logout`、`models`、`usage`、`agent stdio`；没有 `account` | 已观察；命令存在不证明其输出适配器合同 |
| 登录机器事件/身份接口 | `login --help` 有 OAuth/device-auth，但未核实 AutoJev 所需 JSON 事件或机器身份字段 | Unknown / 阻塞；未登录 |
| 模型目录机器输出 | `models` 命令存在；`models --help` 未列 `--json`，未核实输出格式与订阅资格语义 | Unknown / 阻塞；未执行目录读取 |
| 订阅额度 / Extra Usage | `usage` 需要本地 session ID，描述 session token/cost；不是已核实的订阅池/额外消费权限接口 | Unknown / 阻塞；未执行用量读取 |
| 禁止整次调用超额使用的机制 | 没有适用于 AutoJev 请求的已核实接口/执行保证 | Unknown / 阻塞 |
| 当前生产 Grok 生成许可 | UI 有进程内存态、默认关闭、最多 15 次的开关；由于 `login_supported=false`，UI 与 IPC 拒绝启用 | 已观察；开关不是上游身份/额度证据，也不能启动当前 fail-closed 的生产适配器 |
| 许可范围 | 当前连接下所有已核实资格模型和协议；不是 HAND_RUN 中单个模型/协议的运行时绑定 | UI 确认框明确告知账号级范围；实际手测必须严格遵循已记录的单个计划范围 |
| 真实 OAuth、配置/session、账号凭据 | 未访问 | 未测；本次 Agent 执行禁止 |
| 真实模型请求 / credits | 未执行 | 未测；模型数和用量费用为 Unknown，不能填 0 |
| `pnpm test:grok-contract`（含默认关闭、有限预算与 Grok 准入回归） | Rust 64 passed；Vitest 2 files / 49 passed | 通过；只使用 fake/纯函数，不证明真实服务行为 |
| `TAURI_DEV_HOST=127.0.0.1 pnpm test` | 13 files / 90 passed | 通过；离线前端测试 |
| `pnpm build` / `pnpm release:check` | 通过 | 通过；Vite 有 Tauri API 动静态 chunk 与大 chunk 提示 |
| Rust `--lib` 构建（普通 / `isolation-check`）与 `node --check src-tauri/src/isolation-check.js` | 均通过 | 通过 |
| 完整 `cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --lib` | 455 passed / 36 failed | 所有失败来自当前沙箱 `Operation not permitted` socket/process；同一 loopback bind 错误已在干净精确基线复现 |
| `pnpm test:isolated` | 前端构建和 isolation Rust 构建通过；Node 夹具 `listen` 报 `EPERM` | 沙箱限制；在桌面应用启动前失败，未执行 UI 验收或截图 |

预算语义：每个通过完整准入并开始派发的 API 请求最多启动一个 ACP `session/prompt`，占一个有限 dispatch 槽；失败与取消仍计入 dispatch 槽。客户端工具结果续传作为新 API 请求和新 helper turn 单独计数、占另一槽。人工验收需同时记录 `dispatch` 和 ACP `session/prompt` 实际计数，二者均不得超过事先接受的同一个总上限。

## 将来具备条件后填写的验收证据

- 验收负责人及日期/时区：`<填写>`
- AutoJev 完整 commit / branch / build：`<填写>`
- 隔离 OS 用户/环境与应用自有 `HOME`、`GROK_HOME` 证据：`<来源、时间；不能核实时 Unknown>`
- helper 精确 `program` / 版本 / SHA-256：`<填写；不能核实时停止>`
- 用户自行选择的非秘密账号别名：`<填写；不记完整邮箱、密码、token、授权码>`
- 固定模型 / 协议：`<填写>`
- 登录、身份、目录资格、额度接口依据（URL、版本、账号适用范围、时间）：`<填写；缺任一项则阻塞>`
- 整次调用费用/Extra Usage 限制依据（URL、版本、账号适用范围、时间）：`<填写；Unknown 时调用预算=0>`
- 事前批准的最大 dispatch 总数：`<填写整数；不允许扩大>`
- 每协议最大 dispatch、最大工具轮次、最大输出 token、超时：`<逐项填写>`
- 允许的虚构输入 / 工具 / 工具参数 / 固定本地结果：`<填写；不接真实数据或外网>`
- 预计可能费用及来源：`<填写；未知写 Unknown，不能填 0>`

## 观察字段

逐个 dispatch 记录事前计划与本次观察到的实际响应。模型和协议值必须抄录其直接来源（例如响应体 `model` 字段、请求路由、响应 schema 或 `Content-Type`）；身份必须记录已验证身份字段及来源，不能用脱敏账号别名代替。响应或当前连接证据没有提供所需实际值时写 `Unknown`，并停止其余生成步骤。逐字段比较实际值与事前计划；任一不符或 `Unknown` 都不能判通过。

| dispatch # / UTC | 事前计划：model / protocol / 已验证身份 | 实际响应 model：值 / 字段或来源 | 实际身份：值 / 字段或来源（不能用别名代替） | 实际 protocol：值 / 路由或响应来源 | 与计划比较：model / identity / protocol | 响应终态 / HTTP 状态 / 错误码 | 脱敏证据路径 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `<填写>` | `<填写>` | `<值或Unknown；来源>` | `<值或Unknown；来源>` | `<值或Unknown；来源>` | `<match/mismatch/Unknown>` | `<填写>` | `<路径>` |

| 时间 | 流程 | 连接世代 | helper 版本 | 目录资格 | quota source / observed_at | Extra Usage 整次调用权限 | dispatch / helper turns / 工具轮次 | 预算上限 / 实际费用 | 脱敏证据路径 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `<UTC>` | `<填写>` | `<填写/Unknown>` | `<版本/Unknown>` | `<pass/fail/Unknown>` | `<值/Unknown>` | `<prohibited/allowed/Unknown>` | `<实际值/Unknown>` | `<上限；实费/Unknown>` | `<路径>` |

## 当前版本的结果

| 检查项 | 状态 | 观察和来源 | 请求/工具计数 | 脱敏证据路径 |
| --- | --- | --- | --- | --- |
| 生产 Grok 登录和身份 | 阻塞 | 本地 1.0.44 帮助列出登录命令，但机器事件和身份接口未核实；当前应用拒绝启动 helper | 0 | `<留空>` |
| 目录与模型资格 | 阻塞 | `models` 命令存在，但机器输出与资格语义未核实 | 0 | `<留空>` |
| 订阅额度与 Extra Usage | 阻塞 | `usage` 是给本地 session ID 的 token/cost 统计，不能替代账号订阅额度 | 0 | `<留空>` |
| 整次调用额外用量保证 | 阻塞 | 当前无执行约束证据 | 0 | `<留空>` |
| Chat Completions 文本/流式/取消/客户端工具往返 | 未测 | 当前 build 的生产生成入口拒绝；没有发送请求 | 0 | `<留空>` |
| Responses 文本/流式/取消/客户端工具往返 | 未测 | 当前 build 的生产生成入口拒绝；没有发送请求 | 0 | `<留空>` |
| Messages 文本/流式/取消/客户端工具往返 | 未测 | 当前 build 的生产生成入口拒绝；没有发送请求 | 0 | `<留空>` |
| 默认关闭的 Grok 生成许可 | 通过（本地 fake 测试） | 默认 false；有限预算绑定连接世代且不持久化；未知额度/Extra Usage 即使显式 arm 仍拒绝；当前不支持的登录合同使 UI/IPC 均禁用 | 0 | `<留空>` |

## 停止记录

任一请求失败或报告 `failed` 终态时，无论响应是否建议重试，都立即停止剩余步骤并回报；请求/取消终态不明也立即停止。任一身份、模型资格、额度来源/时间、Extra Usage 限制、helper 路径或隔离 home 为 Unknown 时停止；逐请求实际模型、身份或协议与计划不符或为 `Unknown`、用量或费用超限、意外工具、或出现未预期 dispatch 时也立即停止。不重试、不换账号、不换服务商。

- 停止时间与触发条件：`<填写>`
- 停止前的 dispatch / 工具计数：`<填写；不能核实写 Unknown>`
- 真实模型使用量与费用来源：`Unknown` 或 `<可直接观察的数值与来源>`；不得推断为 0
- 脱敏错误/计数证据：`<路径>`
- 汇总结论：`阻塞 / 失败 / 通过`（只在每项所需直接证据齐备时写通过）
- 仍需解阻的上游合同与代码：`<列出接口、版本、字段、来源/时间、整次调用费用边界>`
