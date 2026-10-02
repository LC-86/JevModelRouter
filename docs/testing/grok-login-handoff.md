# Grok 订阅登录交接（Issue #14；当前阻塞）

**状态（2026-10-02）：生产 Grok 登录由本次修复明确禁用。** 旧版本文曾列出真人 OAuth、challenge、identity、退出和换号操作；那些步骤依赖未经证实的 CLI 事件协议，现已撤销。不要照旧版步骤启动 AutoJev Grok 登录。

本分支把生产 `login_supported` 报为 `false`，并在创建 helper home 或启动 `grok login` 前返回 `grok_auth_unverified`。本地替身只用于单元测试，不能打开生产登录。

## 已核实的接口缺口

- 当前 CLI 1.0.44 的 `login --help` 没有 AutoJev 所需的机器可读 challenge/verified-identity 事件协议。
- `account` 不是该版本帮助列出的命令。`models --help` 没列出 `--json`。`usage` 要求本地 session ID，并报告 session token/cost，不能作为订阅用量或 Extra Usage 权限。
- 官方 [CLI reference](https://docs.x.ai/build/cli/reference) 说明 `models` 用于列出可用模型，但没有为本适配器定义账号身份、订阅资格、credits 或机器可读输出 schema。
- [Headless/ACP 文档](https://docs.x.ai/build/cli/headless-scripting) 中的 `session/prompt` 是生成路径；不能用于身份、目录或额度只读探测。

## 安全验证入口

从仓库根目录运行 `pnpm test:grok-contract`。它只运行 Rust 本地 fake-helper 单测和前端纯函数测试，不执行 `grok` CLI、不登录、不访问凭据、不发模型请求或消费额度。结果含义和人工验收状态见 [Grok 人工验收入口](grok-hand-run.md)。

## 解除阻塞所需证据

需要可引用的版本化接口文档或 xAI 支持确认，覆盖：机器可读登录/身份、模型目录及资格、订阅池/Extra Usage 读数，以及整次调用不得产生超额消费的执行约束。现有帮助和公开文档没有提供这些契约。补齐证据前，身份、目录、订阅额度与 extra-use 限制均为 **Unknown**，不执行真人登录/生成验收。
