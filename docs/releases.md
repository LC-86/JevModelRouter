# 桌面发布与在线更新

## 更新链路

正式版使用 Tauri updater，从以下公开地址读取版本清单：

```text
https://github.com/thinkany-ai/autojev/releases/latest/download/latest.json
```

客户端启动后检查一次，此后每四小时检查；也可在「设置 → 关于」手动检查。
用户选择安装后下载并校验签名，安装成功才显示重启入口。开发版与浏览器预览不检查正式版更新。

更新包与 macOS 应用签名是两套机制：Tauri 公钥用于验证更新包来源；Apple Developer ID 与公证用于 macOS 分发。

## 复用 Termany 的签名资料

`pnpm release:mac` 使用与 Termany 相同的环境变量及钥匙串证书：

| 输入 | 用途 |
| --- | --- |
| `APPLE_SIGNING_IDENTITY` | 钥匙串中的 Developer ID Application 身份 |
| `APPLE_ID` | Apple 公证账号 |
| `APPLE_PASSWORD` | Apple 应用专用密码 |
| `APPLE_TEAM_ID` | Apple 开发者团队 |
| `TAURI_SIGNING_PRIVATE_KEY` | 更新签名私钥的路径或内容，可选 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 更新私钥密码；未配置时为空 |

未设置 `TAURI_SIGNING_PRIVATE_KEY` 时，脚本依次读取 `~/.tauri/autojev-updater.key`、`~/.tauri/termany-updater.key`。
仓库中仅保存公钥，当前公钥与 Termany 的配置一致。如果旁边存在 `.key.pub` 文件，本地脚本会检查是否匹配。
不要重新生成或替换已发行版本信任的签名密钥。私钥、证书与密码不进入仓库。

```bash
pnpm release:mac --check
pnpm release:mac
# 指定架构（需先安装对应 Rust target）：
pnpm release:mac --target aarch64-apple-darwin
```

`--check` 仅验证配置与本地签名输入是否存在，不验证证书有效性或 Apple 账号。
正式构建生成已签名、公证的应用、DMG 和更新归档；脚本额外公证、装订并验证 DMG，流程参考 Termany。
默认产物位于 `src-tauri/target/release/bundle/`，指定架构时位于 `src-tauri/target/<target>/release/bundle/`。本地构建不自动发布。

## GitHub Actions 配置

在 `thinkany-ai/autojev` 的 Actions Secrets 中配置：

- `APPLE_CERTIFICATE`：与签名身份匹配的、base64 编码的 `.p12` 证书。
- `APPLE_CERTIFICATE_PASSWORD`：导出该证书时设置的密码。
- `APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`。
- `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（未加密密钥可留空）。

这些资料可复用 Termany 的本地原件或已有组织级 Secrets，并授权给本仓库。GitHub 不允许读取另一个仓库中 Secret 的明文；不能用 `gh secret list` 复制密钥值。
可通过 `gh secret list --repo thinkany-ai/autojev` 核对名称，但名称存在不证明凭据有效。本次本地准备未验证远端 Secrets。

### 下载地址必须公开

GitHub Releases 更新地址需要无需登录即可访问。本次准备未验证远端仓库可见性。
如果源代码仓库保持私有，可使用同组织的公开发布仓库：

1. 配置 Actions 变量 `RELEASE_REPO` 为公开发布仓库名称。
2. 配置 `RELEASE_TOKEN`，授予发布仓库 Contents 读写权限。
3. 修改 `tauri.conf.json` 的 updater endpoint，指向该仓库的 `latest.json`。
4. 如发布仓库默认分支不是 `main`，配置 `RELEASE_BRANCH`。

工作流会核对下载仓库公开性与 endpoint 一致性，不把 GitHub Token 打包到客户端。

## 发布版本

1. 同步 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 中的版本，并更新 `Cargo.lock`。
2. 执行 `pnpm release:check`、`pnpm test`、`pnpm build` 和 `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib`。
3. 提交并推送代码；检查 CI 通过后，推送匹配的版本标签，例如 `v0.1.0`。

标签工作流构建 macOS ARM64 / x64、Windows x64、Linux x64 安装包与签名更新包。各平台依次合并更新清单，避免并发覆盖。
全部构建成功且 `latest.json` 包含四个平台后才将草稿发布为 latest；构建失败保留草稿。手动触发只上传测试产物。

Windows 安装程序当前未使用 Authenticode 签名；所有平台的 Tauri 更新包仍使用更新签名。
首次发布后，使用两个不同版本的签名安装包测试真实升级，包括下载失败、签名失败、重试和重启。单元测试不能替代真实升级验证。

参考：[Tauri updater](https://v2.tauri.app/plugin/updater/)、[macOS 签名](https://v2.tauri.app/distribute/sign/macos/)、[Tauri GitHub Action](https://github.com/tauri-apps/tauri-action)。
