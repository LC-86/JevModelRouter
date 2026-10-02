# R1：实际 CPA 定向验证

规格：[R1 / #52](https://github.com/LC-86/JevModelRouter/issues/52)，父规格 [#51](https://github.com/LC-86/JevModelRouter/issues/51)。这张票是选型验证门；真实来源认证、模型池、DSH 和服务发布分别由 R2–R7 接续。

## 当前结果

实际 CPA 的普通 `/v1/chat/completions` 已证明：在本次虚构 OpenAI-compatible 配置中，唯一 namespace prefix、`force-model-prefix: true` 和每 namespace 单凭据能区分 A1/A2/B1 的同一裸模型。接收端记录包含账号路径、实际 Authorization、裸模型及并发请求标记。该结论不推广到未测试的 OAuth 来源。

| 层次 | 已执行结果 | 尚未完成 |
| --- | --- | --- |
| 固定源码 → 实际 CPA → 假上游 | 通过；24 个并发固定目标，缺失/裸 ID、停用、401/429/500/502/503、冷却及流前断开均无跨账号调用 | OAuth、真实来源资格与费用不在本次试验内 |
| Jev 原生 IPC → 自有 CPA | 首个纵向红灯为 `Command start_cpa_validation not found`；实现后真实 CPA 启动、版本/健康检查通过，退出回收成功 | 最终扩展矩阵待运行 |
| 既有来源界面 → 保存 | 原生表单测试真实 CPA 成功；`cpa-a1` 和对应模型 UUID 已保存进独立临时 DB | 四来源完整保存、重启重读与完整 Jev 网关矩阵仍为 blocked |
| 常规回归 | 前端 91 项、Rust 499 项通过；前端与 isolation-check 构建通过 | 最终提交上的检查以 PR Evidence 为准 |

原生验收停住时，电脑控制工具明确返回 Mac 已锁定、需用户手动解锁。父线程已请求解锁；未把缺少终态报告的运行记为通过。`check-cpa-desktop.mjs` 是待解锁后继续运行的真实桌面验收入口。

本次真实模型调用、登录及账户额度查询均为 **0**。没有读取用户真实凭据，使用空的独立 auth 目录、虚构 API key 和独立临时数据库。所有常规生成费用门禁沿用原实现。

## 制品与运行边界

固定来源、源码和二进制 SHA-256、Go 工具链 SHA-256、v8 管理 API 与 MIT 许可见 [`scripts/cpa-artifact.json`](../../scripts/cpa-artifact.json)。源码未修改，CPA 未链接进 Jev 或复制到仓库。构建脚本将上游原始 `LICENSE` 保存为 `<binary>.LICENSE`。

Jev 只在显式 `isolation-check` 开发构建中拥有该实验服务。启动前校验二进制 checksum；配置验证后绑定 `127.0.0.1`，管理密钥只留后端。普通模型 HTTP 使用独立虚构客户端 key。缺失制品、版本/checksum 不符、占用端口均拒绝启动。

配置拒绝冲突 prefix、多凭据 namespace、共享 alias 池以及同 prefix 静默换来源/账号/套餐/模型。隔离空间的非秘密绑定记录跨服务重启及桌面重读保存历史；移除 namespace 也保留记录。合法重载等待实际模型目录；已应用重载若无法确认就绪，撤销 endpoint 并回收服务。非法配置在写入前拒绝，保留既有服务。

配置关闭 panel 下载、自动更新和服务发现，并传入 `--local-model`。该版本仍会尝试一次 Antigravity 版本网络查询，因此每个 CPA 子进程额外使用 macOS `sandbox-exec`，仅允许回环网络；实际非回环查询被沙箱拒绝。它不修改用户安全设置。服务环境清空，不继承代理、凭据或用户 `.env`。

目录、认证空间、请求和进程归属于本次隔离运行。正常退出等待自有 CPA 回收并移除目录；验收超时只终止该桌面测试独立进程组。验收还放置一个无关进程用于核对回收边界。原仓库 WIP、旧 PR50/worktree 及日常 DSH/profile/VPN 保持原样。

## 复现

从 manifest 的可信来源准备并校验源码与 Go 工具链后，显式执行构建；脚本不会下载、安装或更新系统组件。所有实际制品构建/执行依照平台审批进行。

```sh
sh scripts/build-cpa-validation.sh /absolute/go/bin/go /absolute/cpa.tar.gz /absolute/cpa-pinned /absolute/owned-cache-root
node scripts/check-cpa-selection.mjs /absolute/cpa-pinned
```

selection 脚本校验 checksum 后运行真实 CPA，输出临时证据目录及 `result.json`。失败保留 `failure.log`；结束回收子进程、认证目录和配置。它只证明 CPA 普通 HTTP 边界。

保持 Mac 解锁后执行完整原生链路：

```sh
pnpm build
CARGO_TARGET_DIR=/absolute/owned-target cargo build --locked --offline --features isolation-check --manifest-path src-tauri/Cargo.toml
node scripts/check-cpa-desktop.mjs /absolute/owned-target/debug/autojev /absolute/cpa-pinned
```

原生脚本首次运行通过既有来源对话框配置四个虚构来源模型，重启后核对稳定 UUID；随后验证 Jev 普通模型 HTTP、并发重载、失败零串用、配置拒绝、暂停、取消、超时及进程回收。每次运行清除上一份终态报告并核对独立 run ID。只有两个进程运行都生成各自 `ok: true` 报告，且接收端记录与自有进程检查通过，才算该层完成。锁屏、超时或缺少报告均判失败/受阻。

每个网关 prompt 同时携带该 run ID；接收和取消回执仅取本轮。`src/lib/cpa-desktop-evidence.test.mjs` 离线执行实际验收 JS，以 IPC/HTTP 系统边界夹具验证：已有旧回执时，本轮未接收、未取消或未超时回收均不能成功；旧轮迟到记录不影响本轮并发计数。执行 `TAURI_DEV_HOST=127.0.0.1 pnpm exec vitest run src/lib/cpa-desktop-evidence.test.mjs`。这是验收证据归属回归，不构成 CPA 或原生网关行为通过。

## 实际工作流

已读取原仓库 `AGENTS.md` 和 `docs/agents/issue-tracker.md`；隔离分支从 `949b51c7dd8ca560474d59f53dbad0096e20e599` 开始。实际 Matt Pocock 技能位于 `/Users/cuilei/.agents/skills`：`ask-matt/SKILL.md` 路由到 `implement/SKILL.md`，`tdd/SKILL.md` 用于预先约定的原生 IPC/配置和 HTTP 接缝，`code-review/SKILL.md` 用于 Standards/Spec 两轴独立审查，`pr/SKILL.md` 用于 Draft PR。`thread-rename/SKILL.md` 已执行并读回标题。

Issue 保持 OPEN，Draft PR 的未验范围作为独立审查输入；最终原生矩阵通过前不释放 R2/R3 的 R1 阻塞关系。
