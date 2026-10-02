# Grok 订阅登录交接（Issue #14；当前阻塞）

**状态（2026-10-02）：生产 Grok 登录由本次修复明确禁用。** 旧版本文曾列出真人 OAuth、challenge、identity、退出和换号操作；那些步骤依赖未经证实的 CLI 事件协议，现已撤销。不要照旧版步骤启动 AutoJev Grok 登录。

本分支把生产 `login_supported` 报为 `false`，并在创建 helper home 或启动 `grok login` 前返回 `grok_auth_unverified`。本地替身只用于单元测试，不能打开生产登录。

## 已核实的接口缺口

- 当前 CLI 1.0.44 的 `login --help` 没有 AutoJev 所需的机器可读 challenge/verified-identity 事件协议。
- `account` 不是该版本帮助列出的命令。`models --help` 没列出 `--json`。`usage` 要求本地 session ID，并报告 session token/cost，不能作为订阅用量或 Extra Usage 权限。
- 官方 [公开 commit `72a61251fcffb464bcc687aeb5a998e5a98ec0c9`](https://github.com/xai-org/grok-build/commit/72a61251fcffb464bcc687aeb5a998e5a98ec0c9) 的提交信息标注 `Source-Revision: a549186d9d39311f2d3ee4208db62af8c65aa476`；这是两个不同标识，`a549…` 不是公开 commit SHA。该固定快照的 [`xai-grok-shell/Cargo.toml`](https://github.com/xai-org/grok-build/blob/72a61251fcffb464bcc687aeb5a998e5a98ec0c9/crates/codegen/xai-grok-shell/Cargo.toml) 与 [`CHANGELOG.md`](https://github.com/xai-org/grok-build/blob/72a61251fcffb464bcc687aeb5a998e5a98ec0c9/crates/codegen/xai-grok-shell/CHANGELOG.md) 版本均为 `1.0.16`（2026-09-01），不是本机 CLI 1.0.44；本机版本是否实现同一候选 ACP 契约尚未核实。`x.ai/auth/info` 的 `current_or_expired` 不证明登录有效；`x.ai/auth/check_subscription` 会刷新 JWT，不属于本只读验收可调用的接口。官方 CLI reference 也未给出本机版本对这些方法的兼容承诺。
- [Headless/ACP 文档](https://docs.x.ai/build/cli/headless-scripting) 中的 `session/prompt` 是生成路径；不能用于身份、目录或额度只读探测。

## 安全验证入口

从仓库根目录运行 `pnpm test:grok-contract`。它只运行 Rust 本地 fake-helper 单测和前端纯函数测试，不执行 `grok` CLI、不登录、不访问凭据、不发模型请求或消费额度。结果含义和人工验收状态见 [Grok 人工验收入口](grok-hand-run.md)。

## 解除阻塞所需证据

官方源码已有候选身份、模型目录和 billing 方法，不应表述为“官方没有接口”。仍需核实这些方法是否存在于本机 CLI 1.0.44、其字段与身份/资格语义、版本稳定性，以及是否有能保证整次调用不产生额外消费的机制。billing 数值或自动充值设置本身不证明消费被禁止；本只读验收不得调用会刷新 JWT 的 `x.ai/auth/check_subscription`。这些核实完成前，身份有效性、目录资格、订阅额度与 extra-use 限制仍为 **Unknown**，不执行真人登录/生成验收。
