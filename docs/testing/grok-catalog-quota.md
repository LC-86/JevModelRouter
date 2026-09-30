# Grok 模型目录与额度只读展示（Issue #16）

在 `subscription::SubscriptionAdapter` 只读接缝上实现 Grok 的 `models()`（目录）与 `quota()`（额度），
把当前账号的模型目录、订阅池与额外 credits 只读展示到服务商面板。发现 ≠ 资格：`eligible` 一律 `false`，
生成准入保持既有默认拒绝，本票不放行任何真实调用。

术语：文中的**订阅池**指 `CONTEXT.md` 的「订阅内额度」在本次额度快照里呈现为计量窗口的那一部分
（周期类型、已用百分比、重置时间）；它与「额外用量」是两个计量轴，见 §3。

来源：本仓 GitHub issue #16（`LC-86/JevModelRouter`，见 `docs/agents/issue-tracker.md`）。
本文件内联验收所需的全部契约，不依赖仓库外或未入库文件。

**本文件的字段名与子命令都是本应用的适配约定，未对真实 Grok CLI 验证**（与 `auth.rs:309` 的登录事件契约
同一性质）。真实版本核对与真实额度归 #26 Leo 手测；缺失时只阻止真实启用，不阻塞本票合入。

## 1. 只读辅助进程调用

- 程序解析沿用登录路径：`AUTOJEV_GROK_HELPER`（空格分词 = 程序 + 附加参数），否则 `PATH` 中的 `grok`。
  找不到就是找不到：不下载、不安装、不猜测路径。env 白名单、`GROK_HOME`/`HOME` 指向应用自有 home、
  错误与日志脱敏都与登录路径一致（`subscription/helper.rs`）。
- 只读子命令（本应用约定）：账号 `<extra_args…> account --json`；目录 `<extra_args…> models --json`；
  额度 `<extra_args…> usage --json`。
- `account` 读取服务 `status()`：辅助进程在自己的应用自有 home 里是否已有登录账号、是哪个账号。
  一次刷新最多三次一次性读取（account/models/usage），各自独立超时与回收，**不做跨调用缓存**；
  `status()` 拿不到账号事实时返回错误（`refresh()` 因此不改写任何字段），
  `identity` 为非空字符串 → `state=connected`，为 `null`/缺失 → `state=not_connected`（这是事实，不是失败）。
  `helper_version` 保持 `None`（不探测版本），`account_path` 填应用自有 helper home 路径。
- 一次读取 = 一个一次性进程：spawn → 逐行读 stdout → 收到终态事件或 EOF → 只回收该 pid。
  读取超时 15s；超时、提前退出、无终态事件都是一次**如实失败**，绝不伪造成成功或空结果。
- 隔离验证环境（`crate::runtime::isolated()`）下默认**不得拉起任何辅助进程**（#14 边界不变）：
  隔离里的 Grok 只读刷新必须如实报错，且不改写任何连接字段。
  例外只有一条，且只服务本票的隔离验收：`isolation-check` 构建里 `--autojev-grok-helper <绝对路径>`
  （必须同时 `--autojev-isolated`）显式钉住一个替身程序时，**只读**读取可以用它拉起替身；
  登录路径（`GrokCliAuth`）绝不使用这个入口，隔离下登录仍必须被拒绝。
- 并发读取不得复用登录会话的进程；同一服务商的只读读取串行化，收尾只回收自己那个 pid。

## 2. stdout JSON 行事件（冻结）

每行一个 JSON 对象，`event` 字段决定语义；无法解析或未知 `event` 的行直接忽略（不是失败）。

| event | 形状 | 含义 |
| --- | --- | --- |
| `account` | `{"event":"account","identity":<str\|null>,"source":<str>,"observed_at":<rfc3339 str>}` | 当前辅助进程 home 里的账号标识；`null`/缺失表示未登录 |
| `catalog` | `{"event":"catalog","source":<str>,"observed_at":<rfc3339 str>,"models":[{"id":<str>,"display_name":<str\|null>}]}` | 一次成功目录读取 |
| `quota` | `{"event":"quota","source":<str>,"observed_at":<rfc3339 str>,"plan":{"code":<str\|null>,"name":<str\|null>},"pool":{"period":{"type":<str\|null>,"end":<rfc3339 str\|null>},"used_percent":<number\|null>,"usage_allowed":<bool\|null>},"extra_usage":{"has_credits":<bool\|null>,"unlimited":<bool\|null>,"balance":<str\|null>,"unit":<str\|null>,"permitted":<bool\|null>}}` | 一次成功额度读取 |
| `unsupported` | `{"event":"unsupported","interface":"catalog"\|"quota"}` | 固定版本没有该机器接口；**不是失败** |
| `error` | `{"event":"error","code":<str>,"message":<str>}` | 该次读取失败；`message` 必须过 `helper::redact` |
| `done` | `{"event":"done"}` | 本次读取正常结束 |

- `source` 是稳定的来源标识（如 `grok-cli:models`、`grok-cli:usage`），缺失即 `None`，不臆造。
- `observed_at` 由辅助进程给出（RFC3339）。**失败一律不更新时间**。
- 目录条目缺可用 `id` → 跳过该条目，并把 `models[i].id` 记进 `missing_fields`。
- `display_name` 缺失即 `None`（不伪造名字）。

## 3. 映射规则（禁止编造）

- **订阅池** → 一个桶的 `windows`：`pool.period.type` 作为窗口 `label`（缺失记 `pool.period.type` 并用
  `subscription` 占位）；`pool.used_percent` 仅在有限且 `0 <= x <= 100` 时采用，否则该字段为 `None` 并把
  原始文本记进该窗口的 `invalid_fields`；`pool.period.end` 解析为 Unix 秒写进 `resets_at`（只做表示换算，
  不推算重置时间）。
- **额外 credits** → 同一个桶的 `credits`（`QuotaCredits`）：`has_credits`/`unlimited` 仅在为布尔时采用；
  `balance` 与 `unit` **原样保留字符串**，不解析金额、不换算、缺单位不推断单位；
  `permitted` → `credits.permission`（`true`=allowed、`false`=denied、缺失/null/非布尔=unknown）。
  `extra_usage` 整个缺失或不是对象时，仍必须产出 `credits`（各字段 `None`、`permission=unknown`、
  `missing_fields` 记 `extra_usage`）：界面上这一轴要如实显示为未知，不能整段消失。
- **订阅内许可** → 该桶的 `permission`，来自 `pool.usage_allowed`（缺失/null/非布尔=unknown）。
  它与 `credits.permission` 是**两个不同的计量轴**，互不推导：`credits.permission` 不参与额度状态判定。
- 缺字段逐个记入对应层级的 `missing_fields`；类型不符或越界记入 `invalid_fields` 并保持 `None`，
  绝不截断、补齐或改写成合法值。界面未知一律占位 `Unknown`，不得回退成 `0` 或 `100%`。
- `view` 记为 `grok_cli_usage`，标明消费的是本契约的额度事件。
- 目录成功 → `catalog.state=available` 并以本次结果**整体替换**旧列表（旧列表中不再出现的模型即被撤销），
  同时把「上一次已核实目录里有、本次没有」的 model_id 记进 `removed_models`（顺序稳定去重），
  让「模型移除」与「从未发现」「目录陈旧」「额度失败」分别呈现。
  **本次列表本身不可信时，整份目录读取都不算权威结果**（`models` 数组缺失，或条目缺可用标识被跳过，
  判据只取本次读取自己的 `missing_fields`）：此时不得据此判定「全部被移除」，也不得用空列表覆盖
  已核实目录——本次列表未知既不等于模型被移除，也不等于目录为空。有此前的已核实目录 →
  保留旧模型、旧来源、旧观测时间与旧移除记录，`catalog.state` 记为 `stale`，`missing_fields` 用本次的
  （说明为什么这次没能确认）；此前从未成功读过 → `failed`、列表为空、不记时间；
  目录返回 `unsupported` → `catalog.state=unsupported`、模型列表为空、来源与观测时间**只取本次事件**
  （缺失即 `None`，不沿用上一次的时间：本次没有观测到任何东西）；
  目录读取失败且此前有已核实目录 → `stale` 且原样保留模型（`removed_models` 一并保留）；
  此前没有目录 → `failed` 且列表为空。
- 额度返回 `unsupported` → `quota.state=unsupported`、桶为空；读取失败 → `failed`，保留上一次同账号的桶与
  `observed_at`，并只在**确实保留了历史数字/时间**时 `history=true`。
- 额度状态规则（契约 B，与 #15 一致）：至少一个桶时，任一桶 `denied` → `denied`；全部桶 `allowed`
  且至少一个窗口有有效 `used_percent` → `available`；否则 `unknown`。桶为空 → `unknown`。

## 4. Rust 证据模型（模块拆分后的路径）

`src-tauri/src/subscription/mod.rs` 增加（`CatalogEvidence`/`CatalogRead`/`Quota*` 与 #15 同构）：

```rust
pub enum EvidenceState { Unknown, Available, Stale, Failed, Unsupported, Denied }   // +Denied
pub enum QuotaPermission { Unknown, Allowed, Denied }
pub enum QuotaView { Unknown, RateLimitsByLimitId, RateLimits, GrokCliUsage }       // +GrokCliUsage
pub struct QuotaWindow { label: String, used_percent: Option<f64>, window_minutes: Option<i64>,
                         resets_at: Option<i64>, missing_fields: Vec<String>, invalid_fields: Vec<String> }
pub struct QuotaCredits { has_credits: Option<bool>, unlimited: Option<bool>, balance: Option<String>,
                          unit: Option<String>, permission: QuotaPermission,
                          missing_fields: Vec<String>, invalid_fields: Vec<String> }
pub struct QuotaBucket { limit_id: String, name: Option<String>, plan_type: Option<String>,
                         windows: Vec<QuotaWindow>, credits: Option<QuotaCredits>,
                         permission: QuotaPermission, missing_fields: Vec<String>, invalid_fields: Vec<String> }
pub struct QuotaEvidence { state: EvidenceState, source: Option<String>, observed_at: Option<String>,
                           view: QuotaView, buckets: Vec<QuotaBucket>, missing_fields: Vec<String>, history: bool }
pub struct CatalogRead { state: EvidenceState, models: Vec<DiscoveredModel>, source: Option<String>,
                         observed_at: Option<String>, missing_fields: Vec<String> }
pub struct CatalogEvidence { state: EvidenceState, source: Option<String>, observed_at: Option<String>,
                             removed_models: Vec<String>, missing_fields: Vec<String> }
```

- `SubscriptionAdapter::models()` 返回 `CatalogRead`（不再返回裸 `Vec<DiscoveredModel>`）。
- `Evidence` 增加 `catalog: CatalogEvidence`；`SubscriptionView` 增加 `catalog`。
- `refresh()`：先读状态；读到身份与已核实身份不符 → 整体丢弃并报错；读到身份缺失 → 不改写任何证据并报错
  （不能把已核实身份覆盖成 `None`，也不得把本次目录/额度挂到旧账号名下）。目录与额度**独立读**，任一失败
  不影响另一项写回；写回前复查 provider 仍在、`generation` 与 `identity` 未变。
- `QuotaEvidence`/`Evidence`/`Connection` 不再派生 `Eq`（含 `f64`）。

## 5. 适配器分派（两家互不冒用）

生产 `ConfigStore.subscription` 改为按类型分派的注册表，同时装 Codex 与 Grok 适配器：

```rust
pub struct SubscriptionAdapters { /* Vec<Arc<dyn SubscriptionAdapter>> */ }
impl SubscriptionAdapters {
    pub fn new(adapters: Vec<Arc<dyn SubscriptionAdapter>>) -> Self;
    pub fn single(adapter: Arc<dyn SubscriptionAdapter>) -> Self;   // 兼容既有测试注入
    pub fn for_kind(&self, kind: &ProviderKind) -> Option<&Arc<dyn SubscriptionAdapter>>;
    pub fn supports(&self, kind: &ProviderKind) -> bool;
    pub fn available(&self, kind: &ProviderKind) -> bool;
    pub fn helper_status(&self, kind: &ProviderKind) -> HelperStatus;
}
```

- 所有只读/登录编排按 `provider.kind` 解析适配器；解析不到即明确拒绝（既有 `require_supported` 语义）。
- **per-kind helper 状态**：Grok 行不得显示 Codex 适配器的版本或授权目录（隔离验收已有断言）。
- `--autojev-helper`、`AUTOJEV_GROK_HELPER` 等覆盖入口都不改变这条边界。

## 6. 视图 JSON 形状（前后端冻结）

在既有 `SubscriptionView` 上**只增不改**：

```jsonc
{
  "catalog": { "state": "unknown|available|stale|failed|unsupported",
               "source": "…|null", "observed_at": "…|null",
               "removed_models": ["…"], "missing_fields": ["…"] },
  "quota":   { "state": "unknown|available|stale|failed|unsupported|denied",
               "source": "…|null", "observed_at": "…|null",
               "view": "unknown|rate_limits_by_limit_id|rate_limits|grok_cli_usage",
               "buckets": [ { "limit_id": "subscription_pool", "name": "…|null", "plan_type": "…|null",
                              "permission": "unknown|allowed|denied",
                              "windows": [ { "label": "…", "used_percent": 42.0, "window_minutes": null,
                                             "resets_at": 1799999999, "missing_fields": [], "invalid_fields": [] } ],
                              "credits": { "has_credits": true, "unlimited": false, "balance": "12.50",
                                           "unit": "USD", "permission": "denied",
                                           "missing_fields": [], "invalid_fields": [] },
                              "missing_fields": [], "invalid_fields": [] } ],
               "missing_fields": [], "history": false }
}
```

## 7. 界面（只读展示）

- 既有 `data-testid="sub-status-<id>"` 与「Refresh read-only status」按钮保持不变。
- 新增 `data-testid="sub-catalog-<id>"`：
  `catalog_state=<state> source=<str|Unknown> observed_at=<str|Unknown> models=<id:eligible,…|空> removed=<csv|空> missing=<csv|空>`；
  `removed=` 是上一次已核实目录里、本次结果里已不存在的模型 id 列表。
- 新增 `data-testid="sub-quota-<id>"`：
  `quota_state=<state> permission=<allowed|denied|unknown> source=<str|Unknown> observed_at=<str|Unknown>
  history=<true|false> missing=<csv|空>`，随后每桶一段
  `bucket=<limit_id> windows=<label:used=<n|Unknown>,remaining=<n|Unknown>,minutes=<n|Unknown>,resets_at=<n|Unknown>>
  credits=has:<bool|Unknown>,unlimited:<bool|Unknown>,balance=<str|Unknown>,unit=<str|Unknown>,permission:<allowed|denied|unknown>
  missing=<csv|空> invalid=<csv|空>`。
  一个桶有多个窗口时用 `;` 分隔（窗口内部字段已用 `,`，`;` 避免歧义）；本票的 Grok 契约每个桶只有一个窗口。
  `remaining` 只由 `100 - used_percent` 推导且仅在 `used_percent` 有效时给出，否则 `Unknown`（不得出现 `remaining=0` 的伪造值）。
  未读取过时只输出到顶层 `missing=` 为止，无 `bucket` 段。
- 人类可读面：目录状态、模型条目（`model_id`，`eligible=false` 不得写成可调用）、订阅池窗口的已用/剩余与重置、
  额外 credits 的原文余额与单位、来源与观测时间、缺字段提示；`unsupported`/`unknown` 时显示**官方查看入口**
  （目录：<https://docs.x.ai/build/cli/reference>；额度：<https://docs.x.ai/grok/faq#usage--limits>），
  该入口只作查看，**不作为绕过准入的依据**。
- 历史数据（`history=true`）必须显式标注“历史数据 · 最后成功更新 `<observed_at>`”，保留的数字不清零。

## 8. 验证命令

```sh
pnpm build
pnpm exec tsc --noEmit
pnpm test
pnpm release:check
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
pnpm test:isolated
```

## 9. 证据分层

### 本票可提供的证据

| 层 | 手段 | 能证明什么 |
| --- | --- | --- |
| 映射与状态规则 | `cargo test --lib` 内的单测（真实 spawn 本地假 helper 脚本） | 事件解析、缺字段/越界记录、许可与状态规则、history/stale 语义、身份与世代守卫、超时与回收、注册表分派（Grok 绝不借用 Codex 适配器） |
| 界面纯函数 | `pnpm test`（vitest） | 稳定文本 token 顺序、`Unknown` 不回落成 0、credits 原文与单位不推断、历史文案条件、`eligible=false` 不写成可调用 |
| 端到端（隔离，无替身） | `pnpm test:isolated` 的 `grok` 模式（真实原生窗口 + 真实 IPC） | 隔离下只读刷新**如实失败**、连接字段零改写、界面显示 `catalog_state=unknown`/`quota_state=unknown` 且无编造数字、官方查看入口存在、Grok 行不借用 Codex helper、上游 fixture 零订阅请求 |
| 端到端（隔离，有替身回复） | `pnpm test:isolated` 的 `grok-read` 模式：`--autojev-grok-helper` 钉住 `scripts/fake-grok-read-helper.mjs` | 替身回复的账号/目录/额度经 IPC 落到界面：`available`/`denied`/`unsupported`/`failed+history` 四态、used 与推导 remaining、credits 原文与单位、`removed=`、两个许可轴分开；无编造数字；替身 pid 退出后回收 |

隔离环境**不会**拉起 Grok 辅助进程，因此 `pnpm test:isolated` 走的是“没有机器接口时的诚实未知”分支；
“有辅助进程时的映射”分支由 Rust 单测用本地假 helper 走真实 spawn 覆盖。两者合起来才覆盖 #16 的
“隔离替身回复完成目录／额度界面至后端验收”。

### 不属于本票证据（待 Leo 在 #26 核验）

- 真实 OAuth 登录、真实账号身份与真实 `~/.grok`。本票不发起任何真实登录。
- 真实模型目录内容与真实模型资格：`eligible` 一律 `false`，本票不主张任何模型可调用。
- 真实额度数值、单位、重置时间与 `pool.usage_allowed` / `extra_usage.permitted` 的真实取值。
- `account` / `models --json` / `usage --json` 三个子命令与事件字段在真实 CLI 上的存在性与形状
  （见 §11）。
- 真实额外消费约束：本票不消耗额度、不试探 credits、不发起真实生成；隔离替身不产生任何真实消费，
  因此“不消耗”只有替身侧零派发证据，真实约束由 #26 手测确认。
- 生成准入：`eligible=false` 时先停在 `model_not_eligible`，真实启用仍默认拒绝，本票不放行任何调用。

## 10. 限制

- 三个只读子命令与全部事件字段名都是**本应用的适配约定**，不是已核实的上游行为；与 `auth.rs` 的登录
  事件契约同一性质。
- `remaining` 只由界面用 `100 - used_percent` 推导，不是上游字段。
- 只有一处“来源 + 观测时间”：同一次额度读取产出的桶共用同一个 `source`/`observed_at`；本票不假装
  每个桶有独立时间。
- `credits.permission` 不参与 `quota.state` 判定（两个不同计量轴，见 §3）；它只作只读展示，真实启用
  仍由 #17/#18 的资格与准入决定。
- 隔离验收证明的是替身输入下的映射、状态规则与界面呈现，不证明真实账号行为。

## 11. 待 #26 用真实版本核对（Leo 手测）

1. `grok account --json` / `grok models --json` / `grok usage --json` 是否真实存在、参数与输出是否为
   本契约的形状；若不一致，需同步修改的三处是 `src-tauri/src/subscription/grok.rs` 的映射、
   Rust 单测里的本地假 helper 脚本（`auth.rs` 同类做法的内联脚本）、`src-tauri/src/isolation-check.js`
   的只读诚实性断言。
2. 真实账号身份来源与 `account` 事件是否一致；换号后目录与额度是否绑定新账号（换号重核）。
3. 真实订阅池字段（周期类型、百分比、重置时间）与额外 credits（原文、单位、是否允许消耗）的真实取值。
4. 真实辅助进程 home 下是否只有本应用的凭据；退出/换号后是否清理干净。
5. 证据与官方版本的绑定：本票 `helper_version` 恒为 `None`（探测版本需要拉起辅助进程，与 #14 的取舍一致），
   因此目录/额度证据只绑定应用适配契约，不绑定官方版本号；父规格里「证据绑定版本」的条目尚未实现。
