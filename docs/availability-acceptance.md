# 路由与可用性验收（2026-09-21）

本轮重点是负载均衡、自动选择、故障转移和网关生命周期。正式端口为 **9527**，开发端口为 **9526**。请重启开发版后验收，前端热更新不能替换已运行的 Rust 后端。

## 本轮实现

| 能力 | 行为 |
| --- | --- |
| 负载均衡 | 数字较小的优先级先用；同优先级按权重分配新会话，有会话标识时保持模型。一个候选等价于固定模型。 |
| 故障转移 | 命名的负载均衡、自动选择路由都支持。连接失败、401、402、403、408、429、5xx 在开始向客户端输出前尝试后续候选；每个候选最多一次，默认最多 4 次。400、422 不自动重试。 |
| 熔断恢复 | 默认连续失败 3 次冷却 30 秒；401、402、403、429 立即冷却，采用上游 Retry-After（当前上限 1 小时）。冷却结束只允许一个请求探测，完整响应成功后恢复。旧请求迟到的成功不能清除新熔断。 |
| 超时 | 默认连接 10 秒、响应头等待 60 秒、响应体/流空闲 120 秒，可在系统设置调整。已开始输出的响应不自动重放。 |
| 智能选择 | Jev 连接统一位于「设置 → 网关与路由」。路由只配置均衡、成本、质量或速度偏好，以及本地模型偏好、费用统计基线。 |
| Jev 依赖 | 可配置决策 API 地址和密钥；填写模型 ID 时，地址应为 OpenAI Chat Completions 完整接口。可单独点击「测试 Jev」。只发送请求特征及候选元数据，不发送对话原文。 |
| Jev 降级 | 8 秒内不可用、返回未知候选、未配置密钥时，自动路由回退本地选择；请求记录标明降级原因。单候选不需要 Jev 决策。 |
| 出站网络 | 系统代理、直连、自定义 HTTP(S) 代理；自定义代理绕过 loopback。修改连接超时或代理前需先停止网关。Provider 测试同样使用此配置。 |
| 请求诊断 | 请求详情记录每次尝试的 Provider、模型、HTTP 状态和耗时；系统设置展示渠道健康并每 3 秒刷新，支持手动重置。日志不保存对话正文或密钥。 |
| 模型发现 | 增加 `/v1/models`，返回模型/路由 ID 和显示名；带 Agent 标识时返回其注入目录。 |
| 停止/退出 | 先预检并恢复本实例接管的 Agent 配置，再停止监听，最多等待 5 秒排空请求。恢复冲突会阻止停止/正常退出并报告原因。 |
| 配置保护 | 原始值、注入值、当前值三方合并，只撤销未被用户再次改动的注入字段。支持旧备份迁移。文件和实例锁隔离并发、开发版和正式版。 |
| 桌面后台 | 关闭窗口后保留后台网关；托盘可重新打开或安全退出；安装更新及重启前先执行安全停止。 |
| 崩溃恢复 | macOS/Linux 启动独立看护进程。主进程死亡后尝试恢复其 Agent 配置；已重新启动的实例持有锁时跳过恢复。结果显示在网关设置中。 |
| 模型能力 | 不根据本地工具、视觉或上下文标签拒绝请求，上游决定是否支持。临时测试失败不再永久禁用 Provider。 |

## 建议验收顺序

1. **系统设置**：保存 Jev 地址、模型 ID、密钥，点击测试。模型 ID 留空表示使用 Jev 专用决策协议，填写则使用 Chat Completions 协议，地址需要与协议对应。确认关闭再打开设置后内容保留。
2. **负载均衡**：创建 `auto` 路由，加入至少两个模型，设相同优先级、权重 `1:3`。使用多个新会话检查分配；同一会话不应每轮跳模型。权重按新选择累计，不保证小样本随机比例。
3. **故障转移**：将首选测试渠道置为 429/503，另一个设较低优先级。请求应由备选完成，详情能看到两次尝试；后续新请求在冷却期间跳过故障渠道。
4. **恢复**：恢复测试渠道，等冷却结束后发新会话请求，确认探测恢复。已经绑定备选的会话可以继续留在备选，不强行迁移正在使用的会话。
5. **自动选择**：创建自动选择路由并选两个候选。正常 Jev 决策记录应显示 `jev`；将 Jev 地址改为不可用测试地址后，应仍能本地选择，并记录原因。
6. **流式输出**：检查普通文本和工具调用。模拟流中断后应显示失败，不应偷偷从头调用另一个模型。
7. **安全停止**：连接 Agent 后，修改其不相关设置，再停止网关。原始上游配置应恢复，用户修改应保留。重新启动网关后可用原有选择重新连接 Agent。
8. **实例隔离**：仅停止开发版时，正式版接管的 Agent 不应被恢复；相反亦然。

## 自动验证

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo build --manifest-path src-tauri/Cargo.toml
pnpm test
pnpm build

# 启动浏览器预览；另一终端执行冒烟检查
pnpm dev:web
pnpm exec playwright install chromium
pnpm test:ui

# 已安装 Chrome 时，也可直接使用它
AUTOJEV_BROWSER_CHANNEL=chrome pnpm test:ui
```

浏览器脚本只操作预览数据，覆盖设置保存/重开、自动路由创建/编辑、无页面异常，并生成截图到系统临时目录 `autojev-availability`。可用 `AUTOJEV_PREVIEW_URL`、`AUTOJEV_ARTIFACT_DIR` 指定地址及截图目录。

本轮验证结果：**79 项 Rust 测试、16 项前端测试通过；前端生产构建、桌面 Rust 编译通过；Chrome 浏览器冒烟检查通过。** Rust 测试使用本地模拟上游，覆盖协议转换、权重/优先级、错误状态矩阵、重试上限、冷却跳过、单探针恢复、流式超时、会话绑定、配置冲突、重连期间用户修改保留、实例隔离及父进程被终止后的隔离配置恢复。不调用用户的付费模型。

## 已知边界

- 崩溃恢复无法挽救正在进行的请求；Agent 如果将配置缓存在内存中，需要重新启动或重新加载配置。恢复配置不等于透明网络放行。
- 看护进程当前支持 macOS/Linux，Windows 尚无等效独立崩溃恢复。看护进程与主进程同时被强制终止、系统断电，不保证立即恢复。
- 缺失原始备份、无法解析的配置或用户改过仍指向本端口的关键字段，不能安全猜测原始值；正常停止保留网关运行，崩溃恢复会记录失败供处理。
- 旧版备份没有注入快照，升级迁移只能按已知的接管字段恢复；若升级前已手动修改这些接管字段，请先核对原始备份。新版连接会记录三方恢复所需信息。
- 合并恢复保留配置数据；存在后续编辑时，TOML/YAML/JSON5 的注释或排版可能被规范化。文件完全未变时恢复原始文本。
- 自动故障转移不跨越已开始输出的响应，避免重复内容及重复工具操作。速度/质量偏好依赖配置的模型等级，尚非在线性能测量。
- 本轮没有发布安装包，也没有用真实账号执行付费上游压力测试；托盘交互、系统更新安装及真实 Agent 配置缓存行为仍需桌面验收。

## OpenRouter Jev Decisions

In Settings → Gateway & routing, open the **Jev model** tab. The endpoint is fixed to
`https://openrouter.ai/api/alpha/decisions`, with model `~typesafe/jev-latest` by default.
Enter an OpenRouter API key, click **Test Jev**, then **Save Jev settings**.
Existing settings are retained until explicitly changed; a previously stored key
may belong to a different service and must be replaced in that case.

Automatic routes submit `model`, routing metadata in `state`, and a `questions.route`
choice question whose `criteria` keys are candidate IDs. The gateway reads
`answers.route.choice` and accepts only a listed candidate. Credentials use Bearer
authentication. Conversation text is not sent. Errors or invalid choices fall back
to local selection during routing; the settings probe reports failure instead of
claiming a fallback was a successful Jev response. Legacy custom decision endpoints
and Chat Completions decision models remain supported.

Mock-server tests cover the Decisions request contract, authentication, per-route
preferences, successful selection, unknown candidates, malformed answers, missing
model IDs and HTTP authentication failure. They do not establish live OpenRouter
account access, available balance or decision quality.
