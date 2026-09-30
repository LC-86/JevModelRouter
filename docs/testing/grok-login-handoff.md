# Grok 订阅登录、退出与换号手测交接（Issue #14）

本票建立 Grok 订阅的登录、取消、退出与换号闭环：辅助进程拉起与专用存储、界面视图与轮询，以及
本地清除与远端撤销分开呈现的退出证据。**Agent 侧只在隔离替身与本地假回调下验证**；真实 OAuth／浏览器
登录、真实凭据存储保护、真实远端撤销、真实额度与订阅生成全部未验证，由 Leo 按手测票
<https://github.com/LC-86/JevModelRouter/issues/26> 在真实环境核对。

本文件沿用同目录 [订阅服务商入口与默认拒绝](subscription-entry.md) 与
[统一派发与隔离验证](isolated-dispatch.md) 的写法：Agent 验证与真人手测分开陈述，未验证项不得当成事实。

## 一、Agent 侧已验证的实际结果

以下结果来自本分支的隔离替身、本地假回调、纯函数单测与原生隔离桌面验收，全部在本地跑通。

### 1.1 跑通的命令与实际输出

| 命令 | 实际结果 |
| --- | --- |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib` | 232 passed；0 failed（基线 187，本票新增 45 条：`subscription::auth::lifecycle_tests` 35 + `subscription::helper::tests` 9 + `config::storage_tests` 1） |
| `cargo build --lib` | 完成，0 warning |
| `cargo build --features isolation-check` | 构建通过 |
| `pnpm test` | 12 个测试文件 / 48 条用例全部通过 |
| `pnpm build` | 通过（tsc + vite） |
| `pnpm test:isolated` | exit 0；fresh 与 reload 两遍 `report.ok=true` |

原生隔离桌面新增断言 `grok subscription authorization rejected by isolation, not by helper detection`；
既有断言继续通过：`subscription authorization reports no helper and no revoke`、
`subscription sign-in rejected in isolation`、`subscription sign-out honestly rejected without revoke claims`、
`subscription sign-in UI shows an honest failure without credentials`。上游替身共收到 11 条请求
（ProviderTest 5 / ModelTest 3 / Debug 3），`codex-fixture-model` 与 `grok-fixture-model` 各 0 条，
即订阅模型零派发。

边界：Grok 的端到端证据走 Tauri IPC（`save_provider` + 5 条授权命令 + 快照断言），界面点击链路在
Codex 订阅行验证（两行共用同一 dialog 组件）；独立验证由未参与实现的验证者在本分支上重跑本节命令并核对
结论，见本票 PR 的验证记录（本地临时报告不入库）。

### 1.2 Rust 单测覆盖的语义（逐条对应契约）

Rust 侧只在测试代码中注入 `SubscriptionAuth` 替身，辅助进程的本地假回调按本应用的适配契约输出逐行
JSON 事件（`challenge` / `identity` / `done` / `error`）。以上命令已跑通并覆盖：

1. `begin` 拒绝矩阵：非订阅服务商、已连接（`already_connected`，要求先退出）、隔离环境
   （`helper_isolated`）、辅助进程不可用（`helper_missing`）；成功后 `attempt` 同世代内单调递增、
   世代不变、连接状态为 `AuthorizationPending`。
2. `poll`：`Succeeded{identity}` 写为已连接并记录身份且旧证据作废；`Failed` 回到未连接并保留错误；
   `Cancelled` 回到未连接。`attempt` 或世代不是当前值时返回 `Superseded`，**不写入任何状态**。
3. `cancel`：阶段为已取消、清空 challenge、未连接、**世代不变**；其后的迟到 `poll` 仍是 `Superseded`。
4. `logout`：世代加一、清空身份与证据、清空 helper home 内容、回收该服务商的自有进程，并保留
   provider／model／route 全部配置；返回 `local=Cleared|Failed`，未做真实撤销时 `remote=NotAttempted`
   （绝不写 `Verified`）。
5. `switch_account`：先退出（新世代）再登录；登录失败时保持未连接且身份为空，**不恢复旧账号身份或证据**。
6. 重启后待授权登录不存活：载入配置时把 `AuthorizationPending` 归位为未连接（登录会话是进程内的）。
7. 辅助进程：解析顺序为 `AUTOJEV_GROK_HELPER` 再 `PATH` 中的 `grok`，找不到即不可用，**不下载、不安装**；
   环境在白名单内重建（`PATH`／`TMPDIR`／`LANG`／`TERM` + 显式 `AUTOJEV_GROK_*`（只透传该前缀，见
   `is_helper_override`）），`GROK_HOME` 与 `HOME`
   都显式注入并指向应用自有 home（`ENV_WHITELIST` 已移除 `HOME`，不再沿用进程的日常 home）；隔离环境下
   `spawn` 直接报错（错误含 `isolated`）。
8. 专用存储：`prepare_home` 创建目录并设 `0700`、拒绝 symlink；`cleanup_home` 只清空自家 home 内容
   并保留目录本身；错误、详情与日志都经过 `redact`，不含凭据、token 或辅助进程输出原文（`identity` 与
   `challenge` 按设计原样保留，见第 12 条，用于身份与授权地址核对）。
9. 界面侧纯函数单测：阶段标签、未知错误码回退原文，以及**远端撤销状态不得由本地清除推断**。
10. 稳定 code `helper_unsupported`：本构建只管理 Grok CLI 辅助进程；非 Grok 订阅（Codex）的登录、轮询、
    取消、退出与换号一律以该 code 诚实拒绝，且**零副作用**（不推进世代、不清身份或证据、不写退出证据）；
    其快照 `helper` 恒为 `available=false`、`program`／`version`／`home` 全为 null，`phase=idle`。界面按
    后端原文回退显示真实原因，不把它显示成已支持。
11. 本次返工固化的两条边界：在已连接或未进行中的连接上调用取消一律早返回，**不改写连接状态与世代**；
    同一服务商重新发起登录会先回收上一次的自有辅助进程，再拉起新的尝试。
12. A4 的脱敏边界：辅助进程返回的 `identity` 与 `challenge`（`instructions`／`verification_url`／`user_code`）
    **原样保留、不脱敏**，界面能正常显示身份与授权地址，不会被涂成 `[redacted]`；`redact` 只作用于错误
    `message`／`recovery` 与日志／详情（`local_detail`／`remote_detail`）。这也是 2.6 能核对身份的前提。

### 1.3 原生隔离桌面验收（实际结果）

`pnpm test:isolated` 已跑通（exit 0，fresh 与 reload 两遍 `report.ok=true`），在真实界面中断言：
`subscription_auth` 含该服务商且 `helper.available=false`、`phase=idle`、`logout.remote=not_attempted`；
`begin_subscription_login` 被拒绝且错误含 `isolated` 或 `helper`；隔离环境下 `logout_subscription` 的
拒绝原因是 `helper_isolated`，非隔离的 Codex 行则是 `helper_unsupported`，两者都不报告本地清除成功；
界面点击登录入口后显示失败原因且 DOM 中无凭据字样；上游 fixture 收到的订阅模型生成请求数仍为 0。

### 1.4 复现命令与证据位置

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
pnpm test
pnpm build
pnpm release:check
pnpm test:isolated
```

- 已跑通命令的实际结果见 1.1；`pnpm release:check` 属既有发布检查，本文件不重复记录其输出。独立验证
  由未参与实现的验证者在本分支上重跑本节命令并核对结论，见本票 PR 的验证记录。
- 隔离验收保留脱敏报告、进程日志与请求记录，位置和命名规则见
  [统一派发与隔离验证](isolated-dispatch.md)。

以上证据只证明隔离环境与替身的行为，不证明真实登录、真实凭据存储保护、真实远端撤销或真实生成能力。

## 二、Leo 手测核对项（关联 issue #26）

手测票：<https://github.com/LC-86/JevModelRouter/issues/26>。下列步骤需要真实浏览器与真实账号，
Agent 不代跑。

### 2.1 前置：启动方式、账号与版本

- **真实登录必须用应用本身启动，不要加 `--autojev-isolated`。** `runtime::isolated()` 明确拒绝拉起任何
  辅助进程：隔离模式下登录、取消、退出、换号都会返回含 `isolated` 的拒绝（`helper_isolated`），真实
  OAuth 在该模式下不可能完成。该参数只用于 Agent 的隔离验证，它的价值恰恰是“不拉起真实进程、不联网”。
- 用普通开发构建（`pnpm dev`）或桌面发布版（`pnpm build:desktop` 的产物）启动。普通启动使用日常数据库
  与配置，这是真实手测的代价；如需减少影响，用手测专用的一次性账号，结束后按 2.4 退出、必要时在界面里
  删除该订阅服务商。不要为了“隔离”去复制或篡改日常配置，也不要把辅助进程指向日常 Grok CLI 的共享配置
  目录。
- 应用自有的辅助进程存储是独立的：`<home_dir>/.autojev/subscription-helpers/grok_subscription/<provider_id>/home`，
  与日常 `grok` CLI 默认使用的配置目录分开；`home_dir` 在普通启动下是用户 home。记录实际路径，不要凭
  推测填写。路径里的服务商类型段是 `grok_subscription`（与配置的 serde 形状一致），不是 `grok`。
- 环境隔离（已实现）：辅助进程环境按白名单重建，上游 API 凭据一律清除；`GROK_HOME` 与 `HOME` 都显式
  注入并指向应用自有 home（`ENV_WHITELIST` 已移除 `HOME`，不再沿用进程的日常 home）。仍需实测确认真实
  CLI 确实只用该目录：未触碰 `~/.grok` 或日常 CLI 配置目录（可对照文件时间戳或进程打开的文件）；一旦发现
  CLI 忽略 `GROK_HOME`／`HOME` 而落回日常目录，立即按 2.9 停止并回报。
- 使用一次性／专用账号，不要用日常账号；手测结束后按 2.4 退出并核对清理结果。
- 固定并记录辅助进程来源与版本：记录 `program`，并执行 `grok --version`，或 `AUTOJEV_GROK_HELPER` 指向
  程序的 `--version`，拿到实际版本（该变量允许包含空格分隔的参数，取其 program 部分）。**应用当前不读取
  也不展示辅助进程版本，快照 `helper.version` 恒为“未知”（story 18 未实现）**，不要把它当作已支持的功能。
  固定版本后不要在手测中途更换。
- 本构建只管理 Grok 辅助进程：Codex 订阅行的登录/退出/换号/取消会以 `helper_unsupported` 诚实拒绝
  （零副作用，见 1.2 第 10 条）。手测只覆盖 Grok 订阅行，不要据此判断 Codex 授权能力。

### 2.2 登录（真实 OAuth，未验证能力）

1. Providers 页 Grok 订阅行点“连接／登录”。
2. 核对进入待授权：`phase=pending`，`attempt` 比上次加一，`generation` **不变**；challenge 展示说明与
   `verification_url`／`user_code`。
3. 在浏览器完成授权；回到界面等待轮询。
4. 核对成功态：`phase=succeeded`，`identity` 与刚刚授权的账号一致（见 2.6 身份核对），界面标注“身份已核对”。
5. 核对已连接后再次点“连接／登录”应被拒绝，原因要求先退出。

若第 4 步身份与授权账号不一致，或界面把未知身份显示成已核实，立即按 2.9 停止并回报。

### 2.3 取消

在 `phase=pending` 时点“取消”，核对：`phase=cancelled`、challenge 清空、连接状态回到未连接、
`generation` 不变，且应用自有 home 内容被清空（已实现：取消与 `poll` 终态 `Failed` 都会清空该服务商的
home，见 `GrokCliAuth::cleanup_provider_home`）。随后等待 30–60 秒，核对没有迟到结果把状态改回成功，也
没有新身份写入；并实测确认 home 里没有残留的部分凭据，有残留即按 2.9 停止回报。

### 2.4 退出（本地清除与远端撤销分别核对）

点“退出登录”后**逐项、分别**核对，不得把本地清除当作远端已撤销：

- 本地清除：`logout.local=cleared`（失败则 `failed`）与 `logout.local_detail`；专用 home 目录内容被清空
  且目录本身仍在；`generation` 加一；`identity` 与 `evidence` 清空。
- 远端撤销：单独读 `logout.remote`，取值只能是 `not_attempted`／`failed`／`unsupported`／`verified`。
  当前实现不会把本地清除写成 `verified`；Grok 的真实撤销能力未知，实测值如实记录，不要推测。
- 配置保留：provider／model／route 配置仍在，只有订阅身份被放弃。
- 进程回收：应用自有辅助进程被回收（记录回收到的 pid 列表与回收结果）。

### 2.5 换号

用第二个账号执行“更换账号”（先退出再登录）：

1. 退出后确认 `identity` 已清空、`generation` 已加一。
2. 完成第二个账号登录，确认新的 `identity` 是第二个账号。
3. 若第二步失败或取消：核对界面停在未连接、`identity` 为空、`evidence` 为空，**旧账号身份与证据没有复活**，
   仅有可读的失败原因与恢复动作。

### 2.6 身份核对

- `identity` 必须来自辅助进程返回并写入当前连接；界面、快照与配置文件三处一致。
- 与真实授权账号比对（邮箱／账号标识）。缺失即“未知”，不得推断或编造。
- 换号后旧身份不得出现在任何位置。

### 2.7 存储目标与权限核对

- 路径核对：界面 helper 区的 `home` 与实际目录一致，应为
  `<home_dir>/.autojev/subscription-helpers/grok_subscription/<provider_id>/home`
  （普通启动下位于用户 home 内），不得指向共享的日常 Grok CLI 配置目录。类型段写错（例如按 `grok`
  去核对）会误判成失败。
- 环境归属核对（已实现 + 仍需实测确认）：应用已把 `GROK_HOME` 与 `HOME` 都指向该专用目录并清除上游
  凭据；实测确认真实 CLI 只读写该专用目录，没有触碰 `~/.grok` 或日常 CLI 配置目录（见 2.1）；一旦落回
  日常目录即按 2.9 停止回报。
- 权限核对：目录应为 `0700` 且由应用创建：

  ```sh
  ls -ld <home>
  stat -f '%Sp %N' <home>
  ```

- symlink 拒绝对核：把 home 路径替换为符号链接后重试，应被拒绝（`prepare_home` 拒绝 symlink），
  且不跟随链接写入。
- 辅助进程内生成的凭据文件格式与权限由 CLI 决定，Agent 未验证；实测到的权限如实记录，没有观察到的写“未知”。

### 2.8 观测字段与脱敏证据位置

快照字段 `subscription_auth[]`（每服务商一条，按 `provider_id` 排序）：

| 字段 | 含义 | 手测核对点 |
| --- | --- | --- |
| `provider_id` | 服务商标识 | 只对订阅服务商出现 |
| `phase` | `idle`／`pending`／`succeeded`／`failed`／`cancelled` | 2.2–2.5 的状态流转 |
| `generation` | 连接世代 | 登录不增、退出／换号加一 |
| `attempt` | 同世代登录尝试次数 | 每次登录加一，迟到结果不写 |
| `challenge` | 授权说明、`verification_url`、`user_code` | pending 时展示、取消后清空 |
| `identity` | 已核实账号 | 2.6 |
| `error` | `code`／`message`／`recovery` | 失败时给出可读原因与恢复动作 |
| `helper` | `available`／`version`／`program`／`home` | 存储目标与版本，见 2.1、2.7 |
| `logout` | `local`／`local_detail`／`remote`／`remote_detail` | 2.4，本地与远端分别读 |

脱敏要求：`error.message`／`recovery`、`local_detail`／`remote_detail` 与日志都经过 `redact`。回报与截图
不得包含 token、授权码、完整 `verification_url` 或真实账号全量标识；`user_code` 按需局部遮盖。

证据位置：界面截图（脱敏后）、后端 `eprintln!` 退出回收日志、应用专用 home 与配置文件，以及本票 PR 的
验证记录。

### 2.9 失败停条件

出现下列任一情况立即停止手测，保留脱敏证据与复现步骤：

1. 凭据、token 或授权码出现在界面、DOM 或日志。
2. 隔离环境下仍拉起真实辅助进程或发起真实登录。
3. 未连接时退出被报告为本地清除成功，或本地清除被显示成远端已撤销。
4. `logout.remote=verified` 但没有可指认的真实远端撤销动作。
5. 换号失败后旧账号身份或证据复活。
6. 待授权登录在重启后仍然存活。
7. 订阅模型生成被放行，或生产路径出现可替换替身的开关。
8. 宣称的字段与实际不符（例如 `phase` 与连接状态互相矛盾）。
9. 取消或失败的登录尝试在应用自有 home 里留下部分凭据（已实现清理，实测到残留即失败），或真实 CLI
   读写到 `~/.grok` 等日常目录。

### 2.10 回报格式

```text
环境: commit <sha> / 分支 / 操作系统 / 构建方式（pnpm test:isolated 或手动启动）
辅助进程: program=<路径> version=<CLI 自身输出，应用不展示> home=<路径>
账号: <脱敏标识>
步骤: <2.x 编号>
期望: <契约期望>
实际: <观察到的行为>
字段: phase=<..> generation=<..> attempt=<..> local=<..> remote=<..>
证据: <截图／日志路径，脱敏>
判定: 通过 / 不通过 / 阻塞
```

未知项填“未知”；费用未知不得填 0。判定为“不通过／阻塞”时附最小复现步骤与首次出现该现象的
`generation`／`attempt`。

## 三、明确未验证项

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 真实 CLI 登录参数 | 未验证 | 默认参数 `login` 只是本应用的约定，真实 CLI 的子命令契约未核对，属 issue #26 手测项 |
| 真实 CLI 输出契约 | 未验证 | `challenge`／`identity`／`done`／`error` 逐行 JSON 是本应用的适配契约，真实 CLI 可能输出人类可读文本 |
| 真实 OAuth／浏览器登录 | 未验证 | Agent 只跑隔离替身与本地假回调，未发起真实登录、未读日常凭据 |
| 真实凭据存储保护 | 未验证 | Agent 只验证自建 home 的 `0700` 与 symlink 拒绝；CLI 内部凭据文件的格式与权限未观察 |
| 真实远端撤销 | 未验证 | `logout` 只保证本地清除；未做真实撤销时 `remote` 保持 `not_attempted`，绝不写 `verified` |
| 真实账号身份与模型资格 | 未验证 | 身份来自替身；真实账号的标识、资格与目录未核对 |
| 真实额度证据 | 未验证 | 只读额度读取的真实来源未验证；未知即未知，不填 0 |
| 真实订阅生成 | 未验证（仍默认拒绝） | 生产注入不可用适配器，订阅生成 fail-closed；本票不放行真实生成，也不消耗订阅额度 |
| Codex 订阅授权 | 未实现（`helper_unsupported`） | 本构建只管理 Grok CLI 辅助进程；Codex 的登录/退出/换号/取消/轮询以该 code 诚实拒绝且零副作用 |
| 原生 token 刷新协调 | 未实现 | 本票只有既有的只读证据刷新 `refresh_subscription`（世代变化时丢弃迟到结果）；CLI 侧凭据刷新未实现也未验证 |
| 辅助进程版本可追溯 | 未实现 | 应用不读取、不展示版本，快照 `helper.version` 恒为“未知”（story 18）；版本只能从 CLI 自身输出人工记录 |

以上各条是 issue #26 的手测范围，**不构成本票合入的阻塞条件**；本票的合入依据是第一节的实际隔离验证
（隔离替身、本地假回调、纯函数单测与原生隔离桌面断言），以及这些未验证项在界面与文档中如实呈现。