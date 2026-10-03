# R6：Mac 独立 CPA 开发交付

唯一规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)，本票 [#57](https://github.com/LC-86/JevModelRouter/issues/57)。基线 `a55470711dcdc5163fd263b8f2fb9a84759f8513`，独立 `feat/r6-mac-service` worktree。本交付只使用隔离目录、虚构凭据与回环假上游；真实凭据配置、登录、账号额度查询、模型调用均 **0**。不安装系统服务，不签名/公证，不启用付费 CI，不写日常 DSH/CLI 配置。

## 产物与职责

这是 macOS arm64 的 unsigned development folder，包含 Jev `isolation-check` 原生开发二进制、构建后的前端、独立固定 CPA、源码归档、随附许可和依赖通知、虚构运行入口和 HAND_RUN。不把 CPA 链接进 Jev，不重写 OAuth、刷新或协议内核。只有显式隔离 debug profile 的 Providers 页显示开发服务控制；普通构建没有这些 IPC，真实 CPA 授权仍未配置。

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
CARGO_HOME=/absolute/owned-cargo-cache node scripts/build-mac-development.mjs /absolute/pinned-cpa /absolute/new-development-folder /absolute/owned-target /absolute/verified-cpa-module-cache /absolute/verified-go/LICENSE
```

构建脚本要求干净、已提交 source，设置编译期 `AUTOJEV_DEVELOPMENT_ISOLATION=1`（普通构建不变），执行 `pnpm build` 和 locked/offline 原生 build，核对 source 未改变、两份 Mach-O arm64 文件及 CPA/许可 pin，保留 Node（含 React、字体 OFL）、locked Rust、固定 CPA 模块缓存与 Go 的许可通知，再生成新目录与逐文件 SHA manifest。依赖通知包含构建依赖，不改变 Jev/CPA 自身许可。已存在目录不会覆盖。产物有源码归档与构建命令，可重建/重新验契约；不修改 AGENTS、全局技能或系统配置。

从任意目录运行产物的隔离验收，前端静态资源随附，不依赖开发服务器/node_modules：

```sh
node /absolute/folder/scripts/check-dsh-desktop.mjs /absolute/folder/bin/jev /absolute/folder/bin/cpa --web-root /absolute/folder/web --service-check --capture
```

用 `--manual` 替代 `--service-check --capture` 可打开没有自动生成的隔离桌面：Providers → 隔离 CPA 开发服务 → 启动自有服务；仅接 `a1/a2/b1/paid` 四个虚构 namespace，每个一个虚构凭据，全部上游被 macOS 沙箱限定回环。新临时 home 与 fixture 地址会打印；安全退出用托盘 Quit 或运行器 Ctrl-C。**产物编译时强制隔离；直接运行 `bin/jev` 会在读取普通 profile 前退出。** 手动模式本轮未执行，不冒充桌面自动验收结果。

保存与状态轮询只读自有 child handle，不自动登录、读账号额度或生成。服务管理 key 只留后端，前端/DSH 不得到管理认证或 auth-file。重开后显式启动，保留已存端口、来源身份绑定与稳定模型 UUID；不自动接管端口上的外部服务。

## 故障与恢复

| 状态 | 具体动作 | 不改变的准入事实 |
| --- | --- | --- |
| 文件缺失 / 版本或 checksum 不兼容 | 面板报错；指定 manifest 对应的现成文件，再显式启动 | 不下载更新、不配置真实凭据；校验失败前不启动 CPA |
| 端口占用 | 首次启动可选独立端口；恢复时须由占用者释放保存端口，或新建隔离目录及来源引用 | 不杀、不认领该端口的外部服务 |
| 自有 CPA 退出 | 状态显示退出，撤回该服务 endpoint；点恢复回收旧 child handle，再校验启动 | 固定请求失败，不换来源；恢复不赋予资格或真实生成许可 |
| 安全退出 / 重开应用 | 回收并 wait 自有 CPA；重开显式启动保存端口 | 稳定 UUID、连接实例/世代与 namespace 绑定保留；不自动登录/查询/生成 |
| 身份、模型、协议或费用依据不成立 | 保持拒绝；按 [逐来源 HAND_RUN](relay-hand-run.md)记录未测/禁用与缺失依据 | 不能靠服务恢复、模型选择、provider enabled 或 Debug/测速旁路 |

服务状态是自有进程状态，不能代替来源连接、模型目录、隔离链路或真实验收。API/第三方/Coding Plan 沿用已合入派发及明确 endpoint/身份，既有用户配置不迁移、不扩大。新 CPA 开发服务只允许声明的虚构凭据；真实订阅/Coding Plan 的整次调用费用门禁和身份/模型/协议门禁没有解除。

## 验证范围与移交

TDD 使用 #51 已确认的原生桌面公开操作与最高层网关接缝。自有 CPA 退出后的显式恢复原先报 `Stop the owned CPA before starting another profile`，修复后两次原生进程与 38 次虚构回环请求通过；服务状态入口原先报 `Command get_cpa_development_service not found`，新增面板后覆盖缺文件、错误 checksum/版本、占用端口、退出、恢复与固定请求零派发。普通前端类型构建已通过。最终 SHA 的完整检查、产物与截图证据在交付记录中单独列出，不将中间构建当作最终检查。

复用 R5 的实际固定 CPA 与 DSH 形状文本/多轮/JSON/SSE 工具/取消、固定失败和禁用准入证据；每次 receiver 记录属于当前 run ID。旧 CLI 证据不移作 CPA 成功。实际 DSH 运行、真实身份/资格/协议、OAuth 普通生成固定账号、额度与整次调用费用限制全部未验，统一由 [#58](https://github.com/LC-86/JevModelRouter/issues/58)逐来源回填；[#25](https://github.com/LC-86/JevModelRouter/issues/25)/[#26](https://github.com/LC-86/JevModelRouter/issues/26)仍保留真人未完成事实。

真人事前计划、有限次数/工具/输入输出上限、可能费用、失败即停和脱敏结果格式见 [HAND_RUN](relay-hand-run.md) 与[结果模板](relay-hand-run-result.md)。服务能启动不等于真实来源可调用；本票只交付隔离开发产物和移交入口。
