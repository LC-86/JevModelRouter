# R6：Mac 独立 CPA 开发交付

唯一规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，本票 [#57](https://github.com/LC-86/JevModelRouter/issues/57)。基线 `a55470711dcdc5163fd263b8f2fb9a84759f8513`，独立 `feat/r6-mac-service` worktree。本交付只使用隔离目录、虚构凭据与回环假上游；真实凭据配置、登录、账号额度查询、模型调用均 **0**。不安装系统服务，不签名/公证，不启用付费 CI，不写日常 DSH/CLI 配置。

## 产物与职责

同时交付两种 macOS arm64 unsigned development folder。`--product` 构建普通原生二进制与持久专用 profile 入口：没有 `isolation-check`、没有强制隔离、没有内置虚构授权或 driver。默认构建保留隔离验收产物，用于已合入 R5 的虚构 DSH 全链路回归。两种产物独立 manifest/source/checksum/许可，不共用通过结论。

普通 Providers 页可创建 Codex 或 Grok/xAI CPA 连接，指定固定 artifact，启动/停止/恢复各连接自己的 CPA 子进程与私有配置。CPA 承担 OAuth 与刷新；Jev 只保存凭据引用和非秘密身份、模型绑定。真实生成默认关闭，只有完整、独立审阅且新鲜的证据及有限计划才能进入普通 CPA 模型 HTTP；不会使用旧 CLI helper 许可或管理 token 派发。Grok 套餐/费用依据缺失会阻止调用。Coding Plan 通过独立证据入口启用绑定套餐端点的有限计划；未知事实继续拒绝。
| 产物 | source / version | checksum / 许可 |
| --- | --- | --- |
| Jev | `0.1.2`；完整构建 commit 写入产物 `manifest.json`，并随附 `jev-source.tar` | `bin/jev`、所有 frontend/driver/docs/source 文件 SHA-256 写入 manifest；`JEV.LICENSE` AGPL-3.0-only 与 `NOTICE` 随附 |
| CPA | [`e2bff0107bb307337aaa19018ccddd55f64253d5`](https://github.com/router-for-me/CLIProxyAPI/tree/e2bff0107bb307337aaa19018ccddd55f64253d5)，v8 / `r1-e2bff010`，Go `1.26.0`，darwin/arm64 | 二进制 `b92fd27a406361a409c04e2c9e1ac54f864e7ecfd4ac202b48ef164038586809`；原始 MIT `CPA.LICENSE` SHA-256 `879792e89cf1bdd6a8d446033ec87e30496f97dcafc4656dc53f641509b346a6` |
| CPA 构建输入 | [固定 manifest](../../scripts/cpa-artifact.json) 的源码 tar 与 Go toolchain URL/checksum | [已有复现脚本](../../scripts/build-cpa-validation.sh) 校验 source/toolchain 版本、binary checksum，并复制原始许可；没有下载/系统安装动作 |
| DSH 接口参考 | `0.1.7-rc.1` / pi-ai `0.85.1` | [发布文件 pin](dsh-interface.json)；本轮只回放 HTTP 形状，实际 DSH 运行未验 |

当前仅验证 arm64；Intel Mac、发布 `.app`/DMG、签名、公证和系统长期服务均未验。Jev 构建入口可复现，但没有声称跨机器原生二进制 SHA 必然一致；每次构建独立记录实际 SHA。CPA 的现有复现校验仍要求精确固定二进制 SHA。

## 构建与运行

先有已验证的 CPA 文件和 `<CPA>.LICENSE`、现成 Node 22+/pnpm 10+/Rust/Tauri 工具链及本项目依赖。使用自有 source、Cargo cache/target；锁文件不变，不下载系统软件。将 CPA 放在本任务拥有目录，不能把前序 checkout/target 作为输出目录。

```sh
CARGO_HOME=/absolute/owned-cargo-cache node scripts/build-mac-development.mjs /absolute/pinned-cpa /absolute/new-development-folder /absolute/owned-target /absolute/verified-cpa-module-cache /absolute/verified-go/LICENSE --product
```

构建脚本要求干净、已提交 source。`--product` 不设置强制隔离变量、不编译隔离 feature；不传此选项才设置 `AUTOJEV_DEVELOPMENT_ISOLATION=1` 并编译隔离 feature。执行 `pnpm build` 和 locked/offline 原生 build，核对 source 未改变、两份 Mach-O arm64 文件及 CPA/许可 pin，保留 Node（含 React、字体 OFL）、locked Rust、固定 CPA 模块缓存与 Go 的许可通知，再生成新目录与逐文件 SHA manifest。依赖通知包含构建依赖，不改变 Jev/CPA 自身许可。已存在目录不会覆盖。产物有源码归档与构建命令，可重建/重新验契约；不修改 AGENTS、全局技能或系统配置。

普通桌面使用明确选择的新空目录，或本应用已标记的专用目录。前端静态资源随附，运行无需 node_modules：

```sh
node /absolute/product/scripts/launch-product.mjs /absolute/dedicated-profile
```

添加 `--offline` 则应用外部操作禁用、传输只准回环，CPA 子进程另受 macOS 回环网络沙箱限制。真人计划另行批准后，才在普通入口 Providers → CPA 创建对应连接，指定 `/absolute/product/bin/cpa` 并启动；授权页面由 CPA 返回，Jev 仅打开当前自有会话 URL，不自行实现 OAuth。绝不在日常 profile 或 DSH 配置中验收。

普通公开操作的离线验收运行器（外部注入驱动，不编译进产品）使用新临时目录，验证错误 artifact、端口占用、停止/恢复、自有进程退出及同 profile 应用重开：

```sh
node /absolute/product/scripts/check-product-desktop.mjs /absolute/product/bin/jev /absolute/product/web /absolute/product/bin/cpa
```

原隔离 DSH 全链路产物另用 `check-dsh-desktop.mjs ... --web-root ... --service-check --capture`。只有该隔离产物强制拒绝裸启动；不能将此限制或其 API-key-backed 虚构结果描述为普通 OAuth 产品成功。
保存与状态轮询只读自有 child handle，不自动登录、读账号额度或生成。服务管理 key 只留后端，前端/DSH 不得到管理认证或 auth-file。重开后显式启动，保留已存端口、来源身份绑定与稳定模型 UUID；不自动接管端口上的外部服务。

## 故障与恢复

| 状态 | 具体动作 | 不改变的准入事实 |
| --- | --- | --- |
| 文件缺失 / 版本或 checksum 不兼容 | 面板报错；指定 manifest 对应的现成文件，再显式启动 | 不下载更新、不配置真实凭据；校验失败前不启动 CPA |
| 端口占用 | 首次启动可选独立端口；恢复时须由占用者释放保存端口，或新建隔离目录及来源引用 | 不杀、不认领该端口的外部服务 |
| 自有 CPA 退出 | 状态显示退出，撤回该服务 endpoint；点恢复回收旧 child handle，再校验启动 | 固定请求失败，不换来源；恢复不赋予资格或真实生成许可 |
| 安全退出 / 重开应用 | 回收并 wait 自有 CPA；重开显式启动保存端口 | 稳定 UUID、连接实例/世代与 namespace 绑定保留；不自动登录/查询/生成 |
| 身份、模型、协议或费用依据不成立 | 保持拒绝；按 [逐来源 HAND_RUN](relay-hand-run.md)记录未测/禁用与缺失依据 | 不能靠服务恢复、模型选择、provider enabled 或 Debug/测速旁路 |

服务状态是自有进程状态，不能代替来源连接、模型目录、隔离链路或真实验收。官方/第三方 API 保留已有明确 endpoint、Key 和固定模型路径。Coding Plan 使用独立 `coding-plan.reviewed.json` 入口和校验器；无证据时 shared admission 返回 `coding_plan_unverified`。有限许可绑定来源实例/世代、准确端点、Key 引用、账号、套餐、模型和 Jev artifact；不会借用 CPA OAuth 许可。既有用户配置不迁移。CPA 恢复后许可关闭；旧计划的已用次数与失败锁停保留，不因重开或关闭/开启重置。

## 验证范围与移交

TDD 使用 #51 既定桌面公开操作和最高层网关接缝。普通入口与自有 pinned CPA 生命周期、有限许可/费用未知零派发、实际 CPA 普通 HTTP 接虚构上游、SSE/429/取消失败锁停分别测试。实际 CPA 模型 fixture 是单 Key compatibility；OAuth 管理元数据是替身，不能证明 OAuth 普通生成或真实套餐权益。最终 SHA 的完整检查、manifest、截图/接收端记录在交付证据单独列出。

旧隔离产物的 38 次虚构请求、R5 DSH 文本/多轮/JSON/SSE 工具往返及定向 receiver 记录保持原作用域。真实凭据、OAuth、额度/费用和实际 DSH 均未测、0 次，结果仅回填 [#58](https://github.com/LC-86/JevModelRouter/issues/58)。Coding Plan 的证据输入、校验、有限启用/关闭和固定端点派发属于 #57 工程交付。Grok 套餐/费用、Coding Plan 真实权益及真实协议结果留在 #58；未支持的协议仍明确拒绝。[#25](https://github.com/LC-86/JevModelRouter/issues/25)/[#26](https://github.com/LC-86/JevModelRouter/issues/26)的旧 CLI 证据不移作 CPA 成功。

逐来源计划、有限请求/工具/辅助预算、费用 Unknown 和失败即停要求见 [HAND_RUN](relay-hand-run.md)。来源连接、目录可见、隔离通过、真实通过分别表达；本票与 R7 均不自动关闭。
