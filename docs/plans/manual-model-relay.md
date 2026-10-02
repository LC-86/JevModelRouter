# Mac 本地模型中转站：当前工程入口

**唯一现行行为规格：[Issue #51](https://github.com/LC-86/JevModelRouter/issues/51)。** 本页只提供导航；需求、调用边界和验收条件在该 Issue 维护，研究和 ADR 不另立一套规格。[旧 #10](https://github.com/LC-86/JevModelRouter/issues/10) 保留历史，不再用于开启新阶段实现。

用户于 2026-10-02 明确首个产品目标：在 Mac 接入订阅账号、Coding Plan、官方 API 和 OpenRouter/ZenMux 等第三方来源，形成可区分来源、账号与套餐的模型池，手动选择模型供 DSH 调用。自动分类、评分、智能路由和大统计后置。优先复用成熟认证、刷新和协议能力。

本轮交付规格与拆票；**新功能实现待本轮规格审查收口后启动**。`ready-for-agent` 表示子票已自包含，不代替其阻塞边、真实调用许可或真人验收。

## 依赖与执行入口

| 票 | 交付行为 | 阻塞票 |
|---|---|---|
| [R1 / #52](https://github.com/LC-86/JevModelRouter/issues/52) | 实际 CPA + 假上游验证普通模型 HTTP 精确选择来源/账号，失败不串用 | 无；规格审查收口后开始 |
| [R2 / #53](https://github.com/LC-86/JevModelRouter/issues/53) | 官方 API、第三方、Coding Plan 显式来源配置到固定请求 | R1 |
| [R3 / #54](https://github.com/LC-86/JevModelRouter/issues/54) | CPA 订阅连接、身份/套餐与世代失效闭环 | R1 |
| [R4 / #55](https://github.com/LC-86/JevModelRouter/issues/55) | 同名模型按来源/账号/套餐区分，手选及目录一致 | R2、R3 |
| [R5 / #56](https://github.com/LC-86/JevModelRouter/issues/56) | DSH 固定目标，文本/流式/取消/客户端工具链路 | R4 |
| [R6 / #57](https://github.com/LC-86/JevModelRouter/issues/57) | Mac 服务交付入口与 HAND_RUN 移交 | R5 |
| [R7 / #58](https://github.com/LC-86/JevModelRouter/issues/58) | 逐来源真人授权、权益、费用与 DSH 实机验收 | R6；`ready-for-human` |

R1 完成后 R2/R3 可并行。阻塞关系同时保存在 GitHub 原生依赖；实现按可执行前沿逐票进入新的工作上下文，使用 Matt Pocock `/implement` → `/tdd` → `/code-review`，本轮不启动该阶段。

R1 是选型验证门，不是已完成的服务接入。唯一 prefix 的静态实现不构成固定账号保证；若不成立，验证独立单凭据服务实例。SDK 内部 pinned auth、轮转、session affinity 或同名 alias 均不能替代普通模型 HTTP 的实测。

## 理由、历史与待验

- [R1 实际 CPA 验证与复现](../testing/cpa-r1.md)：制品追溯、普通 HTTP 实证和原生验收状态。
- [复用接口、许可和维护边界](../research/local-relay-reuse.md)：固定 CPA/CPAMP 源码与定向能力限制。
- [独立服务 ADR](../adr/0007-independent-local-relay.md)：复用职责和替换旧 CLI 首选路线的理由。
- [旧工作处置矩阵](2026-10-02-scope-transition.md)：全部旧 Issue、PR49/50、未交付 WIP 和证据去向。
- [术语](../../CONTEXT.md)：来源连接、账号、套餐、发现与可调用目标的区分。

[#25](https://github.com/LC-86/JevModelRouter/issues/25) 与 [#26](https://github.com/LC-86/JevModelRouter/issues/26) 仍 OPEN、真人未完成，当前验收统一移交 R7，Agent 手测资产由 R6 复用。已连接、目录可见、隔离链路通过与真实调用已验证分别表达；最终可用性由 R7 建立。

本轮真实模型调用、登录和账户额度查询为 **0**；未安装/配置真实凭据、未改变权限、未启用付费 CI。当前费用限制继续有效。原执行者工作区、旧分支和本地证据均保留。
