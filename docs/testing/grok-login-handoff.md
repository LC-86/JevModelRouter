# Grok 订阅登录交接（Issue #14；当前阻塞）

**状态（2026-10-02）：生产 Grok 登录由本次修复明确禁用。** 旧版本文曾列出真人 OAuth、challenge、identity、退出和换号操作；那些步骤依赖未经证实的 CLI 事件协议，现已撤销。不要照旧版步骤启动 AutoJev Grok 登录。

本分支把生产 `login_supported` 报为 `false`，并在创建 helper home 或启动 `grok login` 前返回 `grok_auth_unverified`。本地替身只用于单元测试，不能打开生产登录。

## 已核实的接口缺口

- 当前 CLI 1.0.44 的 `login --help` 没有 AutoJev 所需的机器可读 challenge/verified-identity 事件协议。
- `account` 不是该版本帮助列出的命令。`models --help` 没列出 `--json`。`usage` 要求本地 session ID，并报告 session token/cost，不能作为订阅用量或 Extra Usage 权限。
- npm 注册表[精确的 `@xai-official/grok@1.0.44` 记录](https://registry.npmjs.org/@xai-official/grok/1.0.44)返回 HTTP 200、无重定向，含完整 `gitHead` `5b807183dd7978a460f309132cf0d1183d743526` 且无 `repository` 字段。本地安装清单缺少这两个字段；CLI 历史 build ID `5b807183dd79` 与该 `gitHead` 前缀一致。对完整 SHA 的公开 GitHub 查找返回 HTTP 422，仅说明该次查找未解析，不能证明不存在其他映射；本机二进制到可核验官方源码/发布的映射仍未建立。较旧的固定公开快照 [`72a61251fcffb464bcc687aeb5a998e5a98ec0c9`](https://github.com/xai-org/grok-build/commit/72a61251fcffb464bcc687aeb5a998e5a98ec0c9) 标为 1.0.16，其中候选 `x.ai/auth/info` 状态 `current_or_expired` 不证明当前登录有效；`x.ai/auth/check_subscription` 会刷新 JWT，不属于本只读验收可调用的接口。较新的 1.0.45 快照也不是本机 1.0.44 的源码映射；官方 CLI reference 未承诺本机版本支持上述 ACP 方法。
- [Headless/ACP 文档](https://docs.x.ai/build/cli/headless-scripting) 中的 `session/prompt` 是生成路径；不能用于身份、目录或额度只读探测。

## 安全验证入口

从仓库根目录运行 `pnpm test:grok-contract`。它只运行 Rust 本地 fake-helper 单测和前端纯函数测试，不执行 `grok` CLI、不登录、不访问凭据、不发模型请求或消费额度。结果含义和人工验收状态见 [Grok 人工验收入口](grok-hand-run.md)。

## 解除阻塞所需证据

官方源码已有候选身份、模型目录和 billing 方法，不应表述为“官方没有接口”。后续验证某个 helper 可在两条有限路线中选择：使用来源映射可验证的官方固定版本，或在确认 initialize 副作用边界并获得单独授权后，以隔离目录做有界兼容性验证。无需无限期寻找 1.0.44 的完整源码；兼容性验证只记录实际协议行为，不能证明发布来源、账号权益或费用约束。真人登录、账号/额度读取和模型调用仍分别保持关闭，直至用户另行授权。billing 数值或自动充值设置本身不证明消费被禁止；本只读验收不得调用会刷新 JWT 的 `x.ai/auth/check_subscription`。身份有效性、目录资格、订阅额度与 Extra Usage 限制未核实时继续显示 **Unknown**，生成保持禁用。
