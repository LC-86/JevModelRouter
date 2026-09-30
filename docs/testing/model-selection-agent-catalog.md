# 模型选择、稳定标识与 Agent 目录验证（Issue #17 / T07）

本票把订阅模型拆成四件彼此独立的事：**上游目录身份**（`model_id` + `internal_id`）、**用户选择**（`Model.selected`）、**用户停用**（`Model.enabled`）、**当前账号与世代的资格**（`Eligibility`）。选择只决定「是否进入模型列表与自动候选」，停用在后端禁止一切调用（含原 ID 直调），资格不可用只拒绝派发、不改用户配置。Agent 保存目录按 `agent_catalogs` 单独核对，外部待同步只做提示，不推迟后端撤销。

## 界面契约（冻结）

`DashboardSnapshot.subscriptions[].catalog`（由 `subscription::SubscriptionCatalogView` 提供，`lib.rs` 只透传）：

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

## 证据分层

| 层 | 覆盖 | 不覆盖 |
| --- | --- | --- |
| 隔离替身（`pnpm test:isolated` 的 `catalog` 场景） | 发现／建档、未选、勾选、取消选择、停用零派发、同账号目录失败标陈旧、权威移除、换号重核、删行重建、Agent 待同步 | 真实上游目录、真实 OAuth 身份、真实额度 |
| Rust 单元测试（`cargo test`） | 目录状态机（`subscription_catalog`）、准入与候选（`subscription`/`router`/`proxy`/`agent_catalog`）、装配层（`lib.rs`/`agents.rs`） | 原生窗口与真实 IPC 组合 |
| Leo 手测 | 两家真实目录、真实账号资格与额度、界面观感 | — |

## 覆盖矩阵（对应 #17 验收）

| #17 验收 | 证据 |
| --- | --- |
| 1. 新模型未选；取消选择移出列表与自动候选但保留合规直调；停用禁止所有后续调用 | 隔离场景 `discovered`（`selected=false`）、`selected`/`deselected`（公共目录出现→消失，且两次原 ID 直调的拒绝码与状态完全一致）、`disabled`（`model_disabled` + 公共目录移除 + 手动测速报错 + 上游零派发）；Rust：`agent_catalog::build` 要求 `selected`，`catalog_listed`、`generation_ready`、`rule_includes_model` |
| 2. 服务商限定 ID 与内部标识稳定，显示名／邮箱不改变调用目标；上游 ID 变化视为新模型 | 隔离场景 `discovered.internal_id`、`removed`/`disabled`/`switched` 中标识不变、`deletedRow`（删行后重建沿用同一标识）、受控目录把显示名改为 `Catalog Alpha (renamed upstream)` 后 `model_id` 与 `internal_id` 不变；`save_model` 拒绝把目录行的上游 ID／服务商改绑 |
| 3. 同账号目录失败保留已核实项；权威移除或权限撤销不可用；换号后重核且保留选择／停用状态 | 隔离场景 `stale`（保留模型行与配置、`available→stale`、仍按当前世代合格）、`removed`（`removed` + `model_removed`）、`switched`（`unknown`/`account_changed` → 重新登录核对后 `available`/`eligible`，选择与停用保留） |
| 4. 公共目录、应用内选择和 Agent 保存目录分别核对；外部待同步不推迟后端撤销 | 隔离场景逐步读取真实网关 `/v1/models`（无 Agent 头）的公共目录；`pendingSync`（Agent 保存目录与当前选择不一致 → `catalog_pending_sync=true`，此时原 ID 直调结论不变；一旦停用，后端立即以 `model_disabled` 撤销，不等外部配置） |
| 5. 使用既定适配边界的受控目录证明全过程；旧 API 配置迁移与重读通过 | 隔离替身实现 `SubscriptionAdapter`，只替换「上游目录与额度读取」，登录／世代／准入／派发／快照仍是生产代码；旧配置迁移由 `Model.selected` 的 serde 默认值、`subscription_catalogs` 默认为空、以及 `config`／`lib.rs` 的往返测试覆盖 |

## 如何运行

```sh
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
pnpm build
pnpm test:isolated          # 构建前端与 --features isolation-check 的开发二进制，再跑原生桌面验收
```

`pnpm test:isolated` 的 `catalog` 运行把受控目录写入隔离根目录的 `catalog-catalog.json`，并以 `--autojev-catalog-fixture` 交给目录替身；替身按读取顺序前进，顺序与 `isolation-check.js::catalogLifecycle` 的步骤一一对应（发现 → 同账号失败 → 权威移除 → 换号重核 → 删行重建），最后以生产路径注销（专用授权目录随之清空）。每一次原 ID 直调都经真实网关（Debug 入口），断言拒绝码与上游零派发；公共目录由验收替身的 Node 侧代取真实网关 `/v1/models`（页面直接读回环网关会被同源策略挡下），不经过任何应用旁路。

证据留在 `mktemp` 生成的临时隔离目录：`catalog.report.json`（含每一步观察值）、`helper-catalog.jsonl`（替身收到的方法）、`requests.json`（真实上游收到的请求）。目录被替换的替身不会出现在普通开发／发布构建里：`isolation-check` 特性与 `--autojev-catalog-fixture` 开关都只在隔离验收构建中存在，且目录文件必须位于隔离根目录内。

## 已知限制

- 真实上游目录、真实账号资格与真实额度读取仍待 #15／#16／#25 验证；本票只证明「目录状态与用户配置、准入、派发之间的关系」。
- 本构建的订阅生成能力与额度证据都没有接通（`capability_unverified`），因此「取消选择后仍可原 ID 直调」的可观察证据是：**同一请求在已选与未选下的拒绝码与状态完全一致**（选择不参与准入），而不是一次成功生成。成功生成需要能力与额度证据先落地。
- 替身只替换外部目录与额度读取；它不提供真实 OAuth，也不消费任何真实账号额度。
- 换号场景复用同一个受控账号身份，只验证「世代递增 → 资格作废旧目录 → 重核恢复并保留选择／停用」，不验证两家真实账号的邮箱差异。
