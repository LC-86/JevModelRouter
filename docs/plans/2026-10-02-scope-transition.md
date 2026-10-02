# 旧工作处置与新阶段移交

核对基线：初始远端 main `6ead841d30d7e8838e4a9d1d6b22974b975ec5f6`；父线程随后完成 PR49 合并，当前文档分支基于 `5395adda5d90e07c46c1c73e052aacef02e61a12`。核对全部状态旧 Issue **27** 张及全部 PR 列表，以下覆盖全部旧 Issue。现行规格是 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，子票/依赖由[工程入口](manual-model-relay.md)导航。

“保留”指已有成果或决策仍有复用价值；“替换”指新阶段按现行规格改变路线；“后置”不冒称完成；“收口”只描述已实际完成的交付范围。旧 CLOSED 状态保持，真人未验事实保留。

## PR49 / PR50

| 资产 | 处置与价值 | 证据边界 |
|---|---|---|
| [PR49](https://github.com/LC-86/JevModelRouter/pull/49)，head `93e8e61efda2a1beccc14a8f22b0ebd84583ab61` | **已收口**：父线程核对精确 head、既有独立复审和最新 checks 后合并，merge `5395adda5d90e07c46c1c73e052aacef02e61a12`。撤去未验证 ACP 传输、保留静态 source/version gate、文档和隔离截图，符合新目标的默认拒绝与证据口径 | 父线程提供的独立审查无阻断；PR 报告 Rust 499 / 前端 91、隔离 UI 零 helper/模型请求。本轮未重跑。CI 未运行；Bugbot `NEUTRAL` 为额度失败，Codex review 也受额度限制，均不记通过。既有 cached_token 检查不是本轮执行，不证明账号/额度/模型可用 |
| [PR50](https://github.com/LC-86/JevModelRouter/pull/50)，head `1ec9c964ec1b4205714e06de6782fd125dbe04d7` | **保留 Draft、停止扩展、暂不合入**：保留官方 CLI 只读契约、owned-child 取消/超时、字段投影、陈旧/未知 UI 和替身证据；不以完成旧票为由继续建设 CLI 路线 | 旧 head 的取消早于登记 P2 已由作者修复。新 head 作者报告 Rust 503 / 前端 94 / UI 组通过，尚未独立复审；不能写成验收完成。历史 Grok CLI 1.0.44 真实只读成功不是 Jev 成品调用链成功 |

PR50 的未来去留：R1/R3 确认新服务覆盖与维护边界后，若 CLI 只读仍有独立用户价值且维护成本可接受，再独立复审精确 head 和阶段链路；否则由父线程明确按目标替换关闭，保留分支、契约与证据。不为旧票凑合并、不把其中代码机械迁移到 CPA 适配。

## 全部旧 Issue

| Issue / 核对状态 | 处置 | 当前意义或移交 |
|---|---|---|
| [#1](https://github.com/LC-86/JevModelRouter/issues/1) CLOSED：ZenMux 决策供应商 | 保留 / 后置 | 保留已有 Choice 与密钥区分；ZenMux 生成来源由 R2 明确，决策模型不阻塞手选 |
| [#3](https://github.com/LC-86/JevModelRouter/issues/3) CLOSED：订阅接入决策地图 | 保留历史 | 关闭的是旧决策地图，不是成品订阅验收；新方向在 #51 |
| [#4](https://github.com/LC-86/JevModelRouter/issues/4) CLOSED：ChatGPT Pro 契约 | 保留研究 | 固定来源事实仍可查，未证明当前账号可用；CPA 路径由 R3/R7 验证 |
| [#5](https://github.com/LC-86/JevModelRouter/issues/5) CLOSED：SuperGrok 契约 | 保留研究 | OAuth/目录/额度线索不升级为 Jev 或 CPA 真实资格 |
| [#6](https://github.com/LC-86/JevModelRouter/issues/6) CLOSED：授权生命周期 | 部分保留 / 替换 | 身份、隔离、取消/换号边界保留；首选官方辅助进程路线由 ADR0007/CPA 方向替换 |
| [#7](https://github.com/LC-86/JevModelRouter/issues/7) CLOSED：资格与失败接替 | 保留 / 后置 | 发现≠资格、客户端控制和固定失败不转付费保留；智能混合接替后置 |
| [#8](https://github.com/LC-86/JevModelRouter/issues/8) CLOSED：首版验收范围 | 替换范围 | 不再强制两家官方 CLI 各三协议整包才能进入首产品；以 DSH 所需链路分项验收，最终由 R7 建立可用性 |
| [#9](https://github.com/LC-86/JevModelRouter/issues/9) CLOSED：额度与额外用量 | 保留硬边界 | 仅订阅内权益、未知字段/费用明确、整次禁止额外消费证据要求继续有效 |
| [#10](https://github.com/LC-86/JevModelRouter/issues/10) OPEN：旧首版规格 | 替换入口 / 保留历史 | 新 #51 实际存在后再加 superseded 导航；正文和旧验收未完成事实保留，本轮不关闭为完成 |
| [#11](https://github.com/LC-86/JevModelRouter/issues/11) CLOSED：API 统一派发 | 保留 | 网关/dispatcher、回环隔离与现有 API 能力用于 R1/R2/R5 |
| [#12](https://github.com/LC-86/JevModelRouter/issues/12) CLOSED：入口与默认拒绝 | 保留 | 所有入口默认拒绝、稳定错误和测试接缝继续复用 |
| [#13](https://github.com/LC-86/JevModelRouter/issues/13) CLOSED：Codex 登录 | 保留旧兼容资产 / 替换首选路线 | 生命周期/隔离测试有价值；R3 复用 CPA 授权，不再追加官方 helper 功能 |
| [#14](https://github.com/LC-86/JevModelRouter/issues/14) CLOSED：Grok 登录 | 保留替身证据 / 替换路线 | mock 交付不证明真实登录；PR49 生产 gate 保持，R3/R7 建新来源证据 |
| [#15](https://github.com/LC-86/JevModelRouter/issues/15) CLOSED：Codex 目录/额度 | 保留局部能力 | 已有 parser、账号/窗口/缺字段语义可复用；目录仍不等于资格 |
| [#16](https://github.com/LC-86/JevModelRouter/issues/16) CLOSED：Grok 目录/额度 | 保留历史 / 生产状态收口 | 旧替身成果保留，PR49 静态未知状态取代未验证生产路径；PR50 暂不合入 |
| [#17](https://github.com/LC-86/JevModelRouter/issues/17) CLOSED：选择/ID/目录 | 保留并调整 | 稳定 ID、选择/停用区分复用至 R4；新增来源/账号/套餐限定身份 |
| [#18](https://github.com/LC-86/JevModelRouter/issues/18) CLOSED：额度准入 | 保留 | 统一准入、暂停和恢复继续约束新路径；CPA usage 不替代额度依据 |
| [#19](https://github.com/LC-86/JevModelRouter/issues/19) CLOSED：Codex 三协议 | 保留可复用测试 / 后置全集目标 | 协议与流终态替身证据是参考，新增协议转换交 CPA，不继续重写 helper 协议 |
| [#20](https://github.com/LC-86/JevModelRouter/issues/20) CLOSED：Grok 三协议 | 保留历史 / 后置全集目标 | ACP 实现的隔离结果不等于真实能力；源/版本 gate 与最低 DSH 契约继续区分 |
| [#21](https://github.com/LC-86/JevModelRouter/issues/21) CLOSED：Codex 工具 | 保留行为与测试 | 客户端执行、调用 ID/结果关联和取消语义用于 R5，真实 CPA 能力另验 |
| [#22](https://github.com/LC-86/JevModelRouter/issues/22) CLOSED：Grok 工具 | 保留行为与测试 | 保留替身证据和下游控制权；不将旧 ACP 成功扩大到新服务 |
| [#23](https://github.com/LC-86/JevModelRouter/issues/23) CLOSED：显式混合路由 | 保留旧功能 / 后置新扩展 | 不改已有配置；新手选固定目标失败报错，不依赖自动接替 |
| [#24](https://github.com/LC-86/JevModelRouter/issues/24) CLOSED：测试/测速/Debug | 保留 | 共同准入和只读/生成区分复用，后台真实测速不启用 |
| [#25](https://github.com/LC-86/JevModelRouter/issues/25) OPEN：Codex 真人验收 | 移交 / 未完成 | Agent 手测资产给 [R6 / #57](https://github.com/LC-86/JevModelRouter/issues/57)，未完真人项给 [R7 / #58](https://github.com/LC-86/JevModelRouter/issues/58) 的 Codex 行；仍 OPEN |
| [#26](https://github.com/LC-86/JevModelRouter/issues/26) OPEN：Grok 真人验收 | 移交 / 未完成 | 同样由 R6/R7 统一承接 Grok 行；旧 CLI 只读事实不计新链路通过，仍 OPEN |
| [#33](https://github.com/LC-86/JevModelRouter/issues/33) OPEN：可选 secondary 窗口 | 后置 | 旧 Codex 展示非阻断后续修复；无新接入依赖，不放宽额度准入，也不标完成 |
| [#35](https://github.com/LC-86/JevModelRouter/issues/35) CLOSED：helper 元信息串用 | 保留 | 元信息只绑定对应来源、未知不跨服务商回退的修复仍适用 |

本轮对旧票只添加范围/移交导航，不删除历史或改变完成状态。#25/#26 的原始人工清单继续可查，当前执行和结果统一在 #58；旧 CLI 条目若新路径不适用，记录“不适用/目标已替换”，不能勾成真人通过。

## 本地未交付资产

| 资产 | 核对与处置 |
|---|---|
| 原主工作区 `fix-codex-0159-account-login`，HEAD `fc6ff3e79f2f3f8fb78091559ed17dcdcf204844` | 有 11 个已修改文件；未切换、stash、reset、覆盖或提交。4 个文件内容已与当前 main 完全一致：Codex helper、假 app-server、Codex 两份 hand-run/login 文档，避免再次整包合入。其它 7 个仍有差异，保留，未来按具体行为提取，不能从旧基线整包覆盖新 Grok gate |
| 未提交 ADR0003–0006 和 subscription-providers 三份研究 | 共 7 份原文件保留；内容包含旧原生权益/CLI 决议及静态研究。新 ADR0007 明确替换路线，不改写这些原始草稿。旧 Issue resolution 是可追踪的历史依据 |
| 本地 AGENTS.md、CONTEXT.md 与 docs/agents 配置 | 是原工作区的本地工作流资料，未随当前 main 分发；原文件保留并核对哈希。新文档分支仅新增可追踪术语，不修改 AGENTS.md 或全局忽略规则 |
| 8f72 与 t05 worktree | tracked clean，但保留构建/忽略资产；t05 还保留旧 spec、verification、PR 草稿。不清理或归档 |
| PR50 worktree | tracked clean，保留本地 ignored 构建哈希、UI evidence、success/sparse 截图及分支；本轮不追加测试、重建或重写历史 |

原主工作区 18 个改动/未跟踪文件和 5 个本地工作流文件在当前任务目录留有 SHA-256 基线与完成核对结果；仅存路径、状态和哈希，不复制凭据。该快照是保护检查，不是这些 WIP 的独立代码验收。

## 本轮检查与尚未验收

实际执行：全部状态 Issue/PR 与 main 核对、固定公共源码接口/许可阅读、原工作区内容比较、文档链接/状态/依赖走查及项目提交前检查。结果随文档 Draft PR 记录。

未执行：PR50 新 head 独立复审、实际 CPA 定向运行、任何新功能实现、DSH 真实配置/调用、真实登录/账号额度/生成。旧 Agent 测试和父线程 PR49 收口分别标明其来源，本轮不把历史测试数量记为自己的产品验收。本轮全部真实账号/模型动作保持 0。
