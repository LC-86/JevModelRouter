# 模型选择、稳定标识与 Agent 目录验证（Issue #17 / T07）

本票把订阅模型拆成四件彼此独立的事：**上游目录身份**（`model_id` + `internal_id`）、**用户选择**（`Model.selected`）、**用户停用**（`Model.enabled`）、**当前账号与世代的资格**（`Eligibility`）。选择只决定「是否进入模型列表与自动候选」，停用在后端禁止一切调用（含原 ID 直调），资格不可用只拒绝派发、不改用户配置。Agent 保存目录按 `agent_catalogs` 单独核对，外部待同步只做提示，不推迟后端撤销。

## 界面契约（冻结）

`DashboardSnapshot.subscriptions[].catalog_entries`（由 `subscription::SubscriptionCatalogView` 提供，`lib.rs` 只透传）。注意与同级的 `subscriptions[].catalog` 区分：后者是 #15 的只读目录证据（`{ state, source, observed_at, missing_fields }`），前者是逐模型的用户配置与账号资格投影。

```jsonc
{
  "model_id": "上游限定 ID（调用身份）",
  "name": "上游显示名（只作展示）",
  "internal_id": "稳定内部标识 = Model.id",
  "availability": "available | stale | removed | revoked | unknown",
  "eligibility": "eligible | stale | not_discovered | removed | revoked | account_changed | unknown",
  "selected": true,
  "disabled": false
}
```

`Model.selected`（`config.rs`）：旧配置与 API 模型迁移后默认 `true`。
`AgentStatus.catalog_pending_sync`（`agents.rs` + `lib.rs`）：保存的目录与按当前模型／路由／资格重算的目录不一致时为 `true`；没有保存目录时为 `false`。它只作界面提示，准入不看它。

准入拒绝码（稳定，出现在网关响应头 `x-autojev-subscription-denial` 与错误体 `error.code`）：`model_disabled`、`model_not_discovered`、`model_removed`、`model_revoked`、`model_unqualified`，以及既有的连接、能力与额度码。

## 与 #15 目录/额度证据的衔接

- 一次成功的 `models()` 读取就是该账号的完整权威目录：`subscription_catalog::reconcile` 据此整体重核资格，模型行按上游 ID 建档且默认未选。发现本身**不构成**调用资格：适配器报告 `eligible=false`（Codex 契约固定如此：发现 ≠ 资格）时只记为 `unknown`（无资格依据，退出码 `model_unqualified`），不会被显示成「权限已撤销」。`revoked` 保留给能明确报告权限拒绝的适配器；当前两家适配器都不产生该状态。
- 目录读取失败走 #15 的证据保留路径（`catalog.state = stale`、模型列表与时间原样保留），同时 `subscription_catalog::mark_stale` 把该账号同世代的已核实项标陈旧——网络失败不等于模型被移除，资格仍成立。
- 身份不完整（helper 无法确认账号）与明确退出：除 #15 的状态落盘与历史保留外，还会 `invalidate_account`，把目录资格整体作废（`unavailable`／`account_changed`），绝不留下可用的旧账号资格。换号与退出同样递增世代并作废资格，保留用户的选择与停用。

## 证据分层

| 层 | 覆盖 | 不覆盖 |
| --- | --- | --- |
| 隔离替身（`pnpm test:isolated` 的 `model-selection` 运行） | 发现／建档、未选、勾选、取消选择、停用零派发、同账号目录失败标陈旧、权威移除、换号重核、删行重建、Agent 待同步 | 真实上游目录、真实 OAuth 身份、真实额度 |
| Rust 单元测试（`cargo test`） | 目录状态机（`subscription_catalog`）、准入与候选（`subscription`/`router`/`proxy`/`agent_catalog`）、装配层（`lib.rs`/`agents.rs`） | 原生窗口与真实 IPC 组合 |
| Leo 手测 | 两家真实目录、真实账号资格与额度、界面观感 | — |

## 覆盖矩阵（对应 #17 验收）

| #17 验收 | 证据 |
| --- | --- |
| 1. 新模型未选；取消选择移出列表与自动候选但保留合规直调；停用禁止所有后续调用 | 隔离场景 `discovered`（`selected=false`）、`selected`/`deselected`（公共目录出现→消失，且两次原 ID 直调的拒绝码与状态完全一致）、`disabled`（`model_disabled` + 公共目录移除 + 手动测速在派发前报错）。注意本构建里能力与额度证据未接通，替身本来收不到任何生成方法，所以「停用后上游零派发」（`requests.json`）只是支持性检查、单独并不区分停用与否；区分性证据是**同一请求的准入结论从 `capability_unverified` 变为 `model_disabled`**、公共目录移除与测速在派发前被拒。Rust：`agent_catalog::build` 要求 `selected`，`catalog_listed`、`generation_ready`、`rule_includes_model` |
| 2. 服务商限定 ID 与内部标识稳定，显示名／邮箱不改变调用目标；上游 ID 变化视为新模型 | 隔离场景 `discovered.internal_id`、`removed`/`disabled`/`switched` 中标识不变、`deletedRow`（删行后重建沿用同一标识）、受控目录把显示名改为 `Catalog Alpha (renamed upstream)` 后 `model_id` 与 `internal_id` 不变；`save_model` 拒绝把目录行的上游 ID／服务商改绑；显示名只在上游权威目录里更新，且不覆盖用户自定义名 |
| 3. 同账号目录失败保留已核实项；权威移除或权限撤销不可用；换号后重核且保留选择／停用状态 | 隔离场景 `stale`（保留模型行与配置、`available→stale`、仍按当前世代合格）、`removed`（`removed` + `model_removed`）、`switched`（`unknown`/`account_changed` → 重新登录核对后 `available`/`eligible`，选择与停用保留）。适配器契约：一次成功的 `models()` 必须返回完整权威目录，截断读取必须返回 `Err`，否则「目录里没有」会被误判为「被移除」 |
| 4. 公共目录、应用内选择和 Agent 保存目录分别核对；外部待同步不推迟后端撤销 | 隔离场景逐步读取真实网关 `/v1/models`（无 Agent 头）的公共目录；`pendingSync`（Agent 保存目录与当前选择不一致 → `catalog_pending_sync=true`，此时原 ID 直调结论不变；一旦停用，后端立即以 `model_disabled` 撤销，不等外部配置） |
| 5. 使用既定适配边界的受控目录证明全过程；旧 API 配置迁移与重读通过 | 隔离替身实现 `SubscriptionAdapter`，只替换「上游目录与额度读取」，登录／世代／准入／派发／快照仍是生产代码；旧 API 配置迁移由 `Model.selected` 的 serde 默认值、`subscription_catalogs` 默认为空、以及 `config.rs` 的 `ConfigStore` 迁移／重读测试（`legacy_database_config_keeps_api_models_selected_on_reload`）覆盖 |

## 如何运行

```sh
pnpm build
pnpm test
pnpm release:check
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
pnpm test:isolated          # 构建前端与 --features isolation-check 的开发二进制，再跑原生桌面验收
```

`pnpm test:isolated` 的 `model-selection` 运行（`loginMode=model-selection`，与 #15 的 `catalog` 运行分开，互不冒充）把受控目录写入隔离根目录的 `catalog-model-selection.json`，并以 `--autojev-catalog-fixture` 交给目录替身；替身按读取顺序前进，顺序与 `isolation-check.js::modelSelectionLifecycle` 的步骤一一对应（发现 → 同账号失败 → 权威移除 → 换号重核 → 删行重建），最后以生产路径注销（专用授权目录随之清空）。每一次原 ID 直调都经真实网关（Debug 入口），断言拒绝码与上游零派发；公共目录由验收替身的 Node 侧代取真实网关 `/v1/models`（页面直接读回环网关会被同源策略挡下），不经过任何应用旁路。#15 的 `catalog` 运行仍只用替身自己的目录/额度队列（`AUTOJEV_FAKE_HELPER_CATALOG`／`READS`），不会启用这个受控目录替身。

证据留在 `mktemp` 生成的临时隔离目录：`catalog.report.json`（含每一步观察值）、`helper-catalog.jsonl`（替身收到的方法）、`requests.json`（真实上游收到的请求）。目录被替换的替身不会出现在普通开发／发布构建里：`isolation-check` 特性与 `--autojev-catalog-fixture` 开关都只在隔离验收构建中存在，且目录文件必须位于隔离根目录内。

## 已知限制

- 真实上游目录、真实账号资格与真实额度读取仍待 #15／#16／#25 验证；本票只证明「目录状态与用户配置、准入、派发之间的关系」。
- 本构建的订阅生成能力与额度证据都没有接通（`capability_unverified`），因此「取消选择后仍可原 ID 直调」的可观察证据是：**同一请求在已选与未选下的拒绝码与状态完全一致**（选择不参与准入），而不是一次成功生成。成功生成需要能力与额度证据先落地。
- 替身只替换外部目录与额度读取；它不提供真实 OAuth，也不消费任何真实账号额度。
- 换号场景复用同一个受控账号身份，只验证「世代递增 → 资格作废旧目录 → 重核恢复并保留选择／停用」，不验证两家真实账号的邮箱差异。
- 删除模型行（`delete_model`）不会删除目录项与稳定标识：下一次目录核对会按同一 `model_id` 用原 `internal_id` 重新建档（仍为未选）。这是刻意的选择——保留标识可避免已保存的 Agent 引用被悄悄改指；如果希望「删除即隐藏」，需要另开一票定义用户级忽略状态。
- 上游显示名变化会更新服务商目录与未被用户自定义过的模型行显示名；`model_id` 与 `internal_id` 始终不变，调用目标不受显示名影响。
