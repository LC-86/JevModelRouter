# Codex 模型目录与额度只读展示（Issue #15）

在 #12/#13 的 `subscription::SubscriptionAdapter` 只读接缝上实现 `CodexAdapter::models()`（`model/list`）与
`CodexAdapter::quota()`（`account/rateLimits/read`），把当前账号的模型目录与额度桶展示到服务商面板。
发现 ≠ 资格：`eligible` 一律为 `false`，生成准入保持既有默认拒绝，本票不放行任何真实调用。

来源：本仓 GitHub issue #15（`LC-86/JevModelRouter`，见 `docs/agents/issue-tracker.md`）；两个 RPC 的形状来自
固定版本假设 `openai/codex @ ed9e5a26…` 的 `v2/model.rs`、`v2/account.rs`。本文件内联验收所需的全部契约，
不依赖仓库外或未入库文件；工作副本 `tmp/issue-15/spec.md` 是**本地草稿**（`tmp/` 不入库），只作背景，不是法定来源。

## 冻结契约（内联）

### RPC 形状与映射（后端、替身、隔离检查三处一致）

- `model/list`：请求 `{"method":"model/list","params":{}}`。
  响应主形状 `{"data":[{"id","model","displayName","hidden","isDefault",...}],"nextCursor":null}`；
  兼容同一辅助进程的旧形状 `{"models":[...]}`。`model_id` 取 `id`，缺失取 `model`；`name` 取 `displayName`，
  缺失取 `name`；两者都不可用的条目计入 `missing_fields` 的 `model.list[i].id` 并跳过。
  `eligible` 一律保持 `false`（发现 ≠ 资格，资格由 #17 定义），本票不得把发现的模型标成可调用。
- `account/rateLimits/read`：请求 `{"method":"account/rateLimits/read","params":{}}`。
  多桶优先 `rateLimitsByLimitId { "<limitId>": snapshot }`；仅当它缺失或为空时才使用旧版单桶
  `rateLimits: snapshot`。`view` 记为 `rate_limits_by_limit_id` / `rate_limits` / `unknown`。
  固定版本里 `rateLimits` 恒存在（旧版单桶），`ordinaryUsageAllowed` 位于**结果根层**；结果还含
  `accountId`、`rateLimitResetCredits`、`rateLimitUpsell`，本票不消费。
- 窗口字段：`usedPercent`（有限数字且 `0 <= x <= 100`）、`windowDurationMins`（整数 `>= 0`）、`resetsAt`
  （整数 `> 0`，Unix 秒）。越界或类型不符 → 该字段为 `None` 并连同原始文本记入 `invalid_fields`，
  绝不截断或改写成合法值；缺失 → `missing_fields`。
- `credits`：`hasCredits`/`unlimited` 仅在为布尔时采用；`balance` 仅在为字符串时**原样**保留，
  不解析为金额、不推断单位；缺失字段逐个记入 `credits.missing_fields`。
- 许可 `ordinaryUsageAllowed`：布尔 → `allowed`/`denied`；缺失、`null` 或非布尔 → `unknown`。
  固定版本把它放在**结果根层**且为权威；但**桶内显式 `false` 不会被根层 `true` 覆盖**（fail-closed 防御），
  任一桶被显式拒绝即按拒绝处理。
- 每次成功读取记录 `source`（如 `codex-app-server:model/list`、
  `codex-app-server:account/rateLimits/read#rateLimitsByLimitId`）与 `observed_at`（RFC3339 UTC）；
  失败**不得**更新 `observed_at`。

### 额度语义域边界

- 订阅内额度（`windows` 的 `usedPercent`/`windowDurationMins`/`resetsAt`）与额外用量（`credits`）分别表达，
  互不推导：`balance` 只保留上游原文，不换算成金额或 token，不把 `windows` 的百分比折算成 credits。
- 多个桶不跨桶相加：每个桶单独列出自己的窗口与 credits，`permission` 只做全桶聚合，不做数值合并。
- `permission` 的全桶聚合规则：任一桶 `denied` → `denied`；全部桶 `allowed` → `allowed`；否则 `unknown`。

### 状态规则

- `quota.state`：从未读取 → `unknown`；任一桶 `denied` → `denied`；全部桶 `allowed` 且至少一个窗口有有效
  `used_percent` → `available`；否则 `unknown`（`missing_fields` 说明缺什么）。读取成功但既无
  `rateLimitsByLimitId` 也无 `rateLimits` → `unknown`，`missing_fields` 记两个视图键。
- 额度读取失败 → `failed`：失败时保留上一次同账号的桶与 `observed_at`，并只在**确实保留了历史数字/时间**时
  `history = true`（见下文「history 的最终语义」）。
- `catalog.state`：从未读取 → `unknown`；读取成功 → `available` 并以本次结果**整体替换**旧列表
  （旧列表中不再出现的模型即被撤销）；读取失败且此前有已核实目录 → `stale` 且原样保留模型；
  此前没有目录 → `failed` 且列表为空。
- 登出/换号后目录与额度证据一并清空（`unknown`、空列表、无时间）。

### 界面稳定文本（隔离断言读取的接口）

- `data-testid="sub-quota-<provider_id>"`：
  `quota_state=<state> quota_view=<view> permission=<allowed|denied|unknown> source=<str|Unknown>
  observed_at=<str|Unknown> history=<true|false> missing=<csv|空>`，随后每个桶一段
  `bucket=<limit_id> windows=<label:used=N|Unknown,remaining=M|Unknown,minutes=N|Unknown,resets_at=N|Unknown>[,...]
  credits=has:<bool|Unknown>,unlimited:<bool|Unknown>,balance:<str|Unknown> missing=<csv|空> invalid=<csv|空>`。
  `permission` 是全桶聚合（规则见上）。`remaining` 只在 `usedPercent` 有效时由 `100 - usedPercent` 推导，
  否则为 `Unknown`，不得出现 `remaining=0` 的伪造值。从未读取时只输出到顶层 `missing=` 为止，无 `bucket` 段。
- `data-testid="sub-catalog-<provider_id>"`：
  `catalog_state=<state> source=<str|Unknown> observed_at=<str|Unknown> models=<id:eligible,...> missing=<csv|空>`。
- 未知一律 `Unknown` 占位；空列表渲染为空串。人类可读文案（历史数据提示、许可文案）另起节点。

## 证据分层

### 本票可提供的隔离证据

`pnpm test:isolated` 在真实原生窗口、真实 React 控件与 Tauri IPC 下运行，替换服务为
`scripts/fake-codex-app-server.mjs`（只读替身，无网络、无真实凭据）。断言同时取自**界面稳定文本**与
**后端 snapshot**（`invoke('get_snapshot')` 的 `subscriptions[].catalog/quota`），替身 JSONL 只作队列次序佐证：

| 场景（队列名） | 替身产出 | 断言落点 |
| --- | --- | --- |
| `multi` | 多桶 + 旧版单桶同时存在，根层许可 true | `quota_view=rate_limits_by_limit_id`、两桶、`usedPercent/windowDurationMins/resetsAt` 映射、`remaining=100-used`、旧桶不得出现 |
| `single` | 仅 `rateLimits`，根层许可 true | `quota_view=rate_limits`、单桶映射 |
| `missing` | 缺 `resetsAt` 与 `credits`；目录条目无可用标识 | 窗口 `missing_fields` 记 `resetsAt`、桶 `missing_fields` 记 `credits`、跳过条目记 `model.list[i].id`，界面不出现 `resets_at=0` |
| `invalid` | `usedPercent=142`、`resetsAt=0`、`windowDurationMins=-1` | 三字段为 `None` 且原样进 `invalid_fields`；界面 `used=Unknown`、`remaining=Unknown`，不出现 `used=142`/`resets_at=0`/`minutes=-1` |
| `denied` | 根层 `ordinaryUsageAllowed: false` | `permission=denied`、`quota.state=denied`、行内“额度访问被拒绝”文案 |
| `bucket-denied` | 根层 `true` + 桶内显式 `false` | fail-closed：`permission=denied`、`quota.state=denied`，根层 true 不得覆盖 |
| `no-permission` | 根层该键缺失 | `permission=unknown`、`state=unknown`，进 `missing_fields`，不写成 denied 或 0 |
| `null-permission` | 根层该键为 `null` | 同上 |
| `fail-quota` | `account/rateLimits/read` 返回 JSON-RPC 错误 | 此前的读取已成功 → `state=failed`、`history=true`、`observed_at` 与桶数字保持上一次成功值且不被清零、行内显示历史文案 |
| `fail-catalog` | `model/list` 返回 JSON-RPC 错误 | `catalog.state=stale`、旧模型仍列出、`observed_at` 不变；独立失败的额度证据不被改写 |
| 登出 | — | 目录与额度一并清空（`quota.state=unknown`、无 `bucket` 段）；生成仍被统一准入拒绝 |
| 生成准入 | — | 目录可读、额度 available 时（`eligible=false`）与登出后，订阅模型测速一律拒绝，替身从未收到生成方法 |

### 不属于本票证据（待 Leo 在 #25 核验）

- 真实 OAuth 登录、真实账号身份与真实 `~/.codex`。
- 真实模型目录内容、真实额度数值/单位/重置时间、`ordinaryUsageAllowed` 的真实取值。
- 两个 RPC 的真实版本形状（见下文「待核验的版本假设」）。
- 禁止额外消费：本票不主动消耗订阅额度、不试探 credits；隔离替身不产生任何真实消费，因此“不消耗”只有
  替身侧零派发证据，真实约束由 #25 手测确认。

## history 的最终语义

- 额度读取失败时，只有**此前确实成功读过**（保留了同账号的 `observed_at` 或非空 `buckets`）才 `history=true`：
  界面显示“历史数据 · 最后成功更新 `<observed_at>`”，保留的桶数字不清零。
- **首次读取即失败**（此前从未成功读过）→ `state=failed`、`history=false`、`observed_at=None`、`buckets` 为空；
  界面不得出现“历史数据”，也不得伪造 0。
- 覆盖状态：首读失败分支由 `src-tauri/src/subscription.rs` 的 `refresh_tests` 单测覆盖（断言 `history=false`
  且 `observed_at=None`）；隔离队列的首次读取是成功的 `multi`，因此端到端走的是“失败但保留历史”分支，
  首读失败分支端到端**未走**。

## 待核验的版本假设

`model/list` 与 `account/rateLimits/read` 的请求/响应形状是**固定版本假设**，来源为 `openai/codex @ ed9e5a26…`
的 `v2/model.rs`、`v2/account.rs`；已核验的要点：`model/list` 只有 `data`（无 `models` 键）且带 `nextCursor`，
`account/rateLimits/read` 的 `ordinaryUsageAllowed` 位于**结果根层**、`rateLimits` 恒存在。
旧形状（`models` 数组、多桶 map 缺失）按兼容要求保留，替身用独立的 `legacy`/`single` 场景排练兼容分支，
不代表当前版本。

Leo 在 #25 需用真实版本核对：抓取真实 `codex app-server` 的这两个响应，比对键名、嵌套层级与
`ordinaryUsageAllowed` 位置。若不一致，需要同步修改的后端/替身/隔离检查三处分别是：
`src-tauri/src/codex_helper.rs` 的映射、`scripts/fake-codex-app-server.mjs` 的场景产出、
`src-tauri/src/isolation-check.js` 的 catalog 模式断言。

## 验证命令

```sh
pnpm exec tsc --noEmit
pnpm test
pnpm release:check
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
pnpm test:isolated
```

`pnpm test:isolated` 增加一次 `--autojev-login-check catalog` 隔离运行：
`scripts/check-isolated-desktop.mjs` 传入额度队列
`multi,single,missing,invalid,denied,bucket-denied,no-permission,null-permission,fail-quota,fail-quota` 与目录队列
`success,legacy,missing,success,success,success,success,success,success,fail-catalog`，
并在替身侧核对两条队列按次序各被消费一次、替身子进程随桌面退出被回收。

## 限制

- 隔离验收证明的是替身输入下的映射、状态规则与界面呈现，不证明真实账号行为。
- `remaining` 只由界面推导（`100 - usedPercent`），不是上游字段。
- history 文案是界面标签，机器可读依据是 `history=true` 与保留的 `observed_at`；首读失败无历史文案。
- 生成准入在 `eligible` 恒为 `false` 时先停在 `model_not_eligible`（资格由 #17 定义），因此拒绝码
  `quota_denied`/`quota_failed` 在本票的端到端路径上不可达：它们由 `subscription.rs` 单测覆盖，
  隔离验收只断言界面与 snapshot 已展示的 `permission=denied`、`quota.state=denied`/`failed` 与 `history`。
  同理，`view.denial` 只承载连接级原因，不承载额度拒绝码。