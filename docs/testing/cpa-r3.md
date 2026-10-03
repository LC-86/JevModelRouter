# R3：CPA 订阅连接与身份世代

现行规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，实施票 [#54](https://github.com/LC-86/JevModelRouter/issues/54)，基线 `5dd59847cc3afb998a6b747fa5153dfef6394d1a`。这是隔离替身交付；真实 OAuth 登录、真实账户/额度查询和真实模型请求均为 **0**。R7 承接真人验收，R6 承接可交付服务配置。

## 行为与边界

桌面服务商页有独立 CPA 连接面板，公开操作包含添加、发起、检查等待/成功/失败、取消、退出、换号、读取目录、选择、明确重新绑定与删除已退出连接。启动不会登录、查额度或生成。普通构建没有已配置的 CPA 授权服务，界面明确显示未配置；只有 `isolation-check` 在专用临时 root 和显式 `--autojev-cpa-auth-check` 下注入虚构回环管理服务。测试只展示 `.example.invalid` 授权链接，不打开真实授权页面。

`AppConfig.cpa_subscriptions` 保存来源连接与非秘密账号/套餐、连接实例和世代；`cpa_model_bindings` 独立保存固定目标的来源、实例、世代、账号、套餐及原模型 ID。Provider/Model UUID 和 `autojev/model/<UUID>` 沿用已有接口。账号/套餐改变、取消和退出递增世代，旧结果整体丢弃；发现目录不改变保存的模型绑定。新账号必须明确重新绑定，同名 API 不接替。临时读取失败保留历史目录与配置并标陈旧；已观察的身份变化独立于目录读取提交，不因目录失败保留旧身份。

CPA 管理 key、会话 state 和 HTTP 客户端只在后端；桌面只取得授权链接与非秘密投影。只使用固定的 v8 管理操作，没有 `credentials/download`、上传 auth-file、任意 `requests/api-call` 或 token 刷新实现。OAuth/token 存储及刷新属于 CPA；Jev 不读取日常 Codex/Grok CLI 凭据。退出先提交本地拒绝，再取消本连接登记的 session 或删除本连接已认领的 credential reference。清理失败保留清理引用供明确重试，连接保持不可调用；不遍历/删除其它实例凭据，也不终止其它 CPA/CLI 进程。

授权 URL 仍在途时也登记自有任务：取消立即关闭本地连接，但旧发起未收口前禁止再次复用该 profile。固定 CPA 的 `cancelled:false` 可能表示授权已完成；仅在本流程从空的独占 profile 开始、其 session 明确完成且单凭据来源匹配时，才认领非秘密引用用于清理，不恢复连接/资格。清理期间禁止 profile 复用，删除连接须等自有任务清理完成。公开模型编辑不能把 CPA 固定 UUID 改绑来源或上游原 ID；发现、编辑与重启都保留未验证能力，未复制示例模型的上下文或价格。

固定 CPA 的完成 session 只保留一分钟（`oauth_sessions.go` 的 `oauthCompletedSessionTTL`）；已证明归属并持久化的清理引用可以独立于该 session 完成删除重试。若首次认领前记录已经到期，退出将该服务 profile 隔离，清除旧 attempt 并保持本地拒绝；不会认领或删除其中归属未知的凭据。界面明确指引删除旧连接并配置新的专用服务，退休 profile 不再注入新连接；临时管理 HTTP 失败仍保留旧 attempt 供重试。过期响应由管理替身控制，不通过等待或真实授权验收代替。

本票不生成订阅资格、额度、协议能力或整次调用仅用订阅内权益的证据。网关、Debug、服务商/模型测试和测速沿用共同准入；CPA 目标明确返回 `cpa_not_connected`、`cpa_target_identity_changed` 或 `cpa_qualification_unknown`，不走旧 CLI 派发。仅有登录/目录/选择仍零派发。

## 固定接口与逐来源状态

固定源码 `e2bff0107bb307337aaa19018ccddd55f64253d5`，v8 来源、制品与 MIT 许可参见 [R1](cpa-r1.md) 和 [manifest](../../scripts/cpa-artifact.json)。源码查证位置：`internal/api/server_management_v8.go`、`auth_files_v8.go`、`auth_files_provider_oauth.go`、`auth_files.go`。这些是固定版本的接口事实，不是账号资格证明。

| 接口 | 本票用途与证据 |
| --- | --- |
| GET `oauth/auth-url?provider=codex` | 取得 CPA 创建的 URL/state；虚构流程通过 |
| GET `oauth/status?state=…` | 区分 wait/ok/error；虚构流程通过 |
| DELETE `oauth/session?state=…` | 取消登记的 session；不自建 OAuth，迟到结果不能恢复连接 |
| GET `credentials` | 读取单凭据专用 profile 的非秘密元数据；严格过滤账号/套餐字段，忽略原始额外字段 |
| GET `credentials/models?name=…` | registry 发现条目；**不**报告 eligible=true |
| DELETE `credentials?name=…` | 只清除已认领引用；不声明远端账号撤销 |

固定 `credentials` 列表中的 Codex `id_token` 字段是 CPA 提取的 claims 对象；Jev 仅取 `chatgpt_account_id` 与 `plan_type`，不取 JWT 原文、不推断邮箱/默认套餐。目录属于凭据 registry，不证明当前套餐可调用。没有建立普通模型 HTTP 的 OAuth 固定账号保证，也没有启用订阅生成路径。

| 目标订阅来源 | 固定 CPA 授权入口 | Jev R3 状态 | 模型/资格/费用 |
| --- | --- | --- | --- |
| Codex | `StartOAuthV8` 分派 `RequestCodexToken` | 支持已查证管理契约的隔离替身闭环；真实授权待 R7 | 目录替身通过；真实调用、定向 OAuth profile、资格和费用均待验证 |
| xAI | 分派 `RequestXAIToken` | CPA 源码有入口；Jev 待接入/验证 | 未验证，不借用 Grok CLI 资格 |
| Claude、Antigravity、Kimi、kimi-ai、Devin、Meta | 固定 switch 有对应分派 | CPA 源码有入口；Jev 本票不支持这些连接操作 | 未验证，不从 CPAMP provider 枚举推能力 |
| 其它/插件来源 | 取决于实际插件 host；未知来源返回 provider_not_found | 未验证/不支持自动接入 | 不公布模型协议或账号权益支持 |
| 旧 Codex/Grok CLI | 保留既有历史/兼容路径 | 不自动迁移其凭据或改绑到 CPA | 旧证据不自动移用 |

## 验证与存储口径

最高层接缝沿用规格已确认的本地网关、临时 SQLite、公开桌面操作和回环管理替身。已观察：原生 webview 按钮闭环、A→B 换号后的旧固定目标拒绝、失败陈旧目录、显式重新绑定、两次原生进程的保存/重读和新的 run ID 报告。图片为本任务启动的原生窗口，按其 PID 限定截图，未截取其它应用：[原生截图](../screenshots/cpa-r3-native.png)。

2026-10-03 集成提交 `d5bc6e1` 的实现通过前端 96 项、Rust 534 项（其中新增 CPA 行为 9 项与界面投影 1 项），以及构建和 release 检查。[原始桌面报告](cpa-r3-evidence.json) 保留首轮 `ebf73ee4-4fcd-4952-8a1c-1900839ba0c6` 与重启 `8c66f7c1-c39e-4751-becb-c5cec7d9fdf9` 两个 run ID；两轮 `ok:true`、同一 provider/model UUID 与世代 8，接收端模型请求、凭据下载及额度查询均为 0。截图来自后一次重启，服务不自动重连。公共管理/临时 DB 测试覆盖取消迟到授权、换号迟到目录、账号变化且目录失败、待授权进程重启、完成后取消的清理重试、首次认领前到期的安全隔离/删除及授权 URL 迟到时的 profile 复用阻止；原生流程覆盖公开模型编辑不能重指固定来源/原 ID。

隔离 SQLite/重启与应用专用临时路径已验证；**真实 macOS Keychain、CPA OAuth auth-dir 写入/刷新、真实账号和真实额度尚未验证**。本票管理替身使用虚构元数据，未安装真实服务或凭据。CPA R1 的实际 API 定向结果不推广到 OAuth。

```sh
TAURI_DEV_HOST=127.0.0.1 pnpm build
TAURI_DEV_HOST=127.0.0.1 pnpm test
pnpm release:check
CARGO_TARGET_DIR=/absolute/owned-target cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --lib
CARGO_TARGET_DIR=/absolute/owned-target cargo build --locked --offline --manifest-path src-tauri/Cargo.toml --features isolation-check
node scripts/check-cpa-subscriptions.mjs /absolute/owned-target/debug/autojev --capture
```

桌面驱动保留 `first.report.json`、`reload.report.json`、`receiver-records.json` 和本轮 run ID 截图。两份报告均需 `ok:true`，保存的 provider/model/generation 必须一致。缺报告、锁屏、超时均不计通过。清理仅针对该驱动启动的桌面进程组、Vite 和回环服务；截图失败如实输出，不改变产品验证报告。

## R2 合并后的组合回归（d5bc6e1）

已将 PR62 的 main 合并提交 `ca5f24a13da5a9e69164329a1d12429b23ceca28`（R2 验收 head `de57f04f6753de3b8361cd6dccd78d755ae5c192`）集成到 R3 分支。实际文本冲突仅为 `isolation_check.rs` 的脚本选择与 `types.ts` 的 Snapshot 字段：保留两个显式验收入口，并同时保留 API/CPA 的独立投影。自动合并的配置、保存/删除、固定模型校验、Provider 页面与共同准入已复核，两类来源继续使用既有 UUID 和各自身份记录；未改 R2 协议/脱敏实现。

本次重新执行合并后的完整 Rust 534 项（包括 65 秒后台零生成/测速）、前端 96 项、build/release 检查与原生隔离构建。CPA 原生流程把同名 API 反例建为 R2 的显式官方 API 来源：同一 DB 与 Snapshot 同时保存 API 与 CPA，CPA 换号到世代 8 时 API 实例/世代 1 不变，重启保留各自模型 UUID 和绑定；CPA 未授予资格，API 替代请求为 0。两次当前 run ID 与非秘密身份由原始报告保存，没有沿用旧 `e4ab65e` 的通过结果。

R2 原生基线也在此合并结果运行两次，首轮 `97f4d183-cbd8-41d7-9fb2-600fa44c3607`、重启 `d42ad08a-1647-4c9c-a632-8280f07f95f3` 均 `ok:true`，66 次虚构回环请求的固定 endpoint、模型与凭据匹配全部通过；保存/重开、停用/删除、测试、Debug、拒绝与既有三下游文本/SSE 也保持通过。完整报告与非秘密接收记录加入同一原始证据文件。所有运行只使用任务自有临时 DB、虚构 Key 和回环服务；真实登录、账号/额度及模型操作保持 0。

合入顺序由父线程协调 R2 后 R3，合入后需重新验证共有接缝。共享字段集中在 AppConfig/Snapshot 的独立 CPA map，服务商、模型 UUID 与网关准入沿用已有接口。可能交叠的位置包括配置 loader、`save_model`/`validate_model`、服务商删除与页面、Snapshot 和共同准入。

## PR61 的两项 P2 修复

复现基线为 `d5bc6e133ac997309be062bd31bc9d6d48dedf7a`，Codex review `5400758293`；本轮只处理取消迟到清理与删除后 UUID 漂移。

- [取消与轮询重叠](https://github.com/LC-86/JevModelRouter/pull/61#discussion_r4173157765)：现有公开管理/回环测试新增清理责任断言后实际失败，取消返回时旧 poll 未结束但 `cleanup_finished` 已为 true。修复后在途 poll 保留清理责任，阻止连接删除/复用；迟到完成或残留不明时隔离旧专用服务，不认领或删除未证明归属的凭据，也不恢复身份。被隔离配置不能再用于新连接。
- [删除后重新发现](https://github.com/LC-86/JevModelRouter/pull/61#discussion_r4173157773)：原生 webview 调用公开 `delete_model` 后点击读取目录，UUID 从 `cd212aff-4636-4687-8663-b577b897d3e5` 变为 `f2ad9c32-dbb5-4c8a-a2e6-3b8321a98781`，断言实际失败。连接现在持久保存原 ID 到 UUID 的非秘密记录，公开删除也为既有配置补记 UUID；两处共用连接的 UUID 登记方法；重新发现沿用 UUID，默认未选择，已有绑定引用保持可解析，换号仍须明确重新绑定。

修复代码重新通过 Rust **538/538**、前端 **96/96**、build/release 与原生隔离构建。原生首轮 `866470dc-1c49-4236-9bf7-c5585f080fee`、重启 `c13fc7b8-07f9-4469-bb8d-abf21352438a` 均 `ok:true`，覆盖未选择及已绑定模型的公开删除/目录重建、明确重新选择与重启。两次删除均恢复 UUID `8dedb686-093a-4e0f-858d-7ec7d7868175`；已绑定模型恢复时 `bound:true / selected:false`。临时读失败、换号拒绝旧目标和同名 API 零接替继续通过。

[原始报告](cpa-r3-evidence.json) 的 `pr61-p2-fixes` 保存失败与通过报告、接收记录；上述 R2 原生 66 次记录仍标记为 `d5bc6e1` 历史组合回归，没有冒称本轮重跑。当前截图更新到本轮重启的自有原生窗口。普通等待/取消驱动等待检查按钮重新可用，取消重叠由上述回环竞态测试覆盖。真实登录、账号/额度和模型操作继续为 **0**；本票没有真实账号验收。

## 13:21 的 Waiting 取消回归

[新增 P2](https://github.com/LC-86/JevModelRouter/pull/61#discussion_r4173333931) 的复现基线为 `6094b395038a3b186208d242739a5e07a85480a1`。公开回环测试实际失败：成功取消后迟到的 Waiting 没有创建/认领凭据，却被一概隔离，导致无法重试授权。

现在由同一自有 attempt 记录取消回执和已结束的 Waiting。两者都结束、取消明确成功且专用空间确认空时保留服务并结束清理；先结束 Waiting 时仍保留责任等取消回执，取消 HTTP 失败则保持清理待重试。迟到完成、取消未确认、残留凭据、目录元数据读取失败或授权状态到期保持安全隔离，不认领未知凭据、不恢复身份。

同一取消接缝的 **13/13** 回归覆盖两种响应顺序、取消失败显式重试、Waiting 的可重试与不确定残留、迟到完成/到期；完整 Rust **538/538**、前端 **96/96**、build/release 与原生隔离构建通过。原生公开 poll 在途时点击取消，再释放 Waiting；报告保存 `service_available:true / stage:cancelled / account:null / plan:null / retry_stage:waiting`，随后既有目录/选择/网关拒绝、两次删除 UUID 保留及重启继续通过。原始证据 `pr61-p2-fixes.cancel-waiting` 和当前两轮报告保存这些结果；真实操作仍为 **0**。

原两项在 `6094b395` 完成独立 Standards/Spec 复审。按父线程最新指示，本次 Waiting 功能修复只作必要回归，不新增复审轮次或低影响重构；不把旧 head 的独立审查结论移用到最终新 head。

## 13:51 的两 mutex 循环 P1 核对

[审查意见](https://github.com/LC-86/JevModelRouter/pull/61#discussion_r4173427257) 的基线为 `b480b45a78f5ebcf79986238c092f94adcedb36a`。实际 `CleanupLease` 保存 `&Mutex<HashSet<String>>` 和连接 ID；`cleanup_lease` 插入清理标记使用的临时 guard 在返回前已释放。`disconnect` 跨管理等待持有的是逻辑清理责任，进入 `cleanup_owned` 后取 attempts 锁时没有持 cleaning guard。begin/remove 的嵌套顺序是 attempts → cleaning，没有审查所述 cleaning → attempts 的反向嵌套。父线程独立只读核对确认此 P1 为误报；本轮生产锁逻辑保持原样。

新增公开管理/临时 DB/虚构回环回归以独立于 Tokio 的 std mpsc 接收端和 10 秒墙钟期限验证：取消 HTTP 回执挂起、清理 lease 存活时，begin 已返回前次清理的拒绝；释放回执后清理结束；32 组同时 begin/disconnect、每组清理和末尾待授权 shutdown 完成。测试在未改动的生产代码上直接通过，实测 0.11 秒。失败路径使用 runtime shutdown_background，避免阻塞 worker 导致测试 teardown 无限等待。不伪造失败复现，也不把这里写成修复了生产死锁。

原生首轮 `3cb499be-da70-45fc-8f67-9d3bcfbdc143` / 重启 `7edf3557-7bec-4a5e-aef0-1661475edf56` 均 ok:true。Waiting 取消后保留服务、再次授权进入 waiting，两次删除保留 UUID `8b52355d-8b65-46bc-bc32-fc425e6273a1` 和绑定，重启保持引用；模型/下载/账户查询均 0。本轮测试与文档之外没有生产文件变更，前端96/build/release/native构建属于 b480b45 已通过的相同生产树。

完整 Rust 两次各为 **538 passed / 1 failed**：第一次未改动的旧 CLI 替身测试 `a_superseded_attempt_never_reclaims_the_current_attempt` 报 helper_exited，第二次未改动的 `exclusive_file_lock_prevents_recovery_race` 报 WouldBlock；两项分别定点重跑通过，失败原因未确认。新有界并发回归在两次完整运行都通过。原始证据 `pr61-p1-lock-check` 保留结果，不冒称全量539通过；按父线程指示不扩展旧 CLI/文件锁修复或复审轮次。真实操作仍为 0。
