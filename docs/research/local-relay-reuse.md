# CLIProxyAPI 与管理层复用边界

核对日期：2026-10-02 UTC。此文记录固定源码的接口事实和选型依据；现行行为与验收条件以 [#51](https://github.com/LC-86/JevModelRouter/issues/51) 为准。未运行 CPA/CPAMP、未登录、未查询真实账号或模型；运行证明由 [R1 / #52](https://github.com/LC-86/JevModelRouter/issues/52) 承接。

## 选择与职责

优先使用 **CLIProxyAPI 独立本地服务**，复用其已有认证、凭据刷新、上游执行和协议转换。Jev 保留连接身份、用户手选、稳定目录、统一准入和服务生命周期；既有 API 派发继续按已验证路径复用。CPAMP 用作管理接口和状态投影参考，本阶段不引入它的管理服务器、持久统计或 dashboard，也不搬移两套源码。

| 来源 | 固定快照与许可 | 维护/兼容事实 |
|---|---|---|
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI/tree/e2bff0107bb307337aaa19018ccddd55f64253d5) | `e2bff0107bb307337aaa19018ccddd55f64253d5`；[MIT](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/LICENSE) | [go.mod](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/go.mod) 是 v8 模块，Go 1.26.0；下载的是源码快照，未选择/安装发行 artifact |
| [CPA-Manager-Plus](https://github.com/seakee/CPA-Manager-Plus/tree/05ebb7f275dbe575211cb886436d4b99936c1cd9) | `05ebb7f275dbe575211cb886436d4b99936c1cd9`；[MIT](https://github.com/seakee/CPA-Manager-Plus/blob/05ebb7f275dbe575211cb886436d4b99936c1cd9/LICENSE) | [README](https://github.com/seakee/CPA-Manager-Plus/blob/05ebb7f275dbe575211cb886436d4b99936c1cd9/README_CN.md) 推荐 CPA v7.3.4+；这不是该 CPAMP 对 CPA v8 的完整兼容证明 |

GitHub 仓库元数据在本次读取时均为未归档；CPA `pushed_at=2026-10-02T18:32:23Z`，CPAMP `2026-10-01T09:08:20Z`。近期提交不保证接口稳定；固定快照、发行 artifact 和运行契约分别核对。现有 Jev 是 AGPL-3.0-only；分发 CPA 二进制或参考实质源码时保留各自 LICENSE/版权通知，具体产物声明由 R6 核对。本轮没有复制上游实现或更改许可。

## 实际接口

固定 CPA [模型路由注册](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/server_routes.go) 提供 `/v1/models`、`POST /v1/chat/completions`、`POST /v1/responses` 和 `POST /v1/messages`。这说明接口存在，不证明任一账号、模型或字段的真实兼容。

| 用途 | 固定源码中的接口/数据 | 不能推导的结论 |
|---|---|---|
| 连接管理 | [v0 管理路由](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/server_management.go)：auth-files、status/fields、provider auth-url、get-auth-status、oauth-session/callback；[v8 路由](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/server_management_v8.go) 有新命名 | 不同 provider 不保证相同授权方式或身份字段；保留 v0 不等于所有 v7 行为已验证 |
| 非秘密连接投影 | [auth_files.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/handlers/management/auth_files.go) 列表含 id/auth_index/name/provider、状态及部分账号、quota 资料 | 列表中出现身份/目录不能授权生成；前端只消费必要字段，不下载 auth-file |
| 单凭据目录 | 同文件的 GetAuthFileModels 通过 name/ID 查管理器后返回 `registry.GetModelsForClient` | 是注册表目录，不是实时账号套餐资格；同 filename 对应虚拟多连接时也需检查歧义 |
| 管理代理请求 | [api_tools.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/handlers/management/api_tools.go)：`POST /v0/management/api-call` 用 auth_index 与 token 占位进行管理侧 HTTP | 不能把它当 DSH 的生成入口；这会把任意 HTTP/管理权限和上游协议耦合进 Jev |
| 请求统计 | [usage.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/api/handlers/management/usage.go) 的 usage queue；CPAMP 的 usage 持久化/分析 | 请求 token/成本/成功数不等于套餐剩余量，也不能证明仅订阅内消费 |

CPA [配置模板](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/config.example.yaml) 的 server.host 默认空字符串会监听所有接口。独立本地服务须显式 loopback、管理认证留在后端，并关管理 panel 下载/后台更新等本阶段不需要的功能；不能原样拷贝默认模板就称隔离。模板同时明确 v8 写入可能迁移旧字段，接口适配和配置更新须由 R1 固定验证。

## 普通模型请求能否固定来源/账号

这是最低可行接入的阻塞契约，当前结论是 **有可复用机制，运行保证待验证**。

1. [service_models.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/sdk/cliproxy/service_models.go) 的 `applyModelPrefixes` 按 credential prefix 生成带前缀目录；开启 force-model-prefix 时通常不再暴露裸 ID。[file synthesizer](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/watcher/synthesizer/file.go) 从各 auth metadata 读 prefix，拒绝包含内部斜线的 prefix。
2. [conductor_selection.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/sdk/cliproxy/auth/conductor_selection.go) 依据每 auth 的注册模型和 `authSupportsRouteModel` 筛选；[conductor_models.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/sdk/cliproxy/auth/conductor_models.go) 只剥离匹配该 auth 的 prefix。**推断：**每有效连接唯一 namespace、单凭据及无碰撞 alias 可以形成普通模型 HTTP 的定向路径；仍须实际服务验证选择、失败、重载及并发。
3. [handlers_context.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/sdk/api/handlers/handlers_context.go) 有 `WithPinnedAuthID`；[model_execution.go](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/sdk/api/handlers/model_execution.go) 的内部请求有 AuthID。静态核对未找到普通模型 HTTP 把 `auth_index` 绑定该 context 的公共契约。内部 SDK 能力不能写成独立服务已经提供的普通 HTTP 参数。
4. 配置支持 round-robin/fill-first/session affinity；session affinity 在绑定不可用时会接替。共享 alias、同 prefix、多 Key group 都可能保留多 credential 候选。`request-retry=0` 只禁额外重试轮，不能据此断言初始轮不遍历其它凭据。

R1 使用实际 CPA 和回环假上游，先验证唯一 prefix + 一个有效 credential/namespace，记录命中的来源/账号及其它接收端零请求；覆盖同名模型、缺号、停用、401/429/5xx、冷却、并发、重载和取消。若 prefix 不成立，验证独立单凭据服务实例；两种方案均无法证明时该 provider 保持不可调用。无需为解决选择问题重新建设认证或协议内核。

## CPAMP 可以参考什么

- [authFiles API](https://github.com/seakee/CPA-Manager-Plus/blob/05ebb7f275dbe575211cb886436d4b99936c1cd9/apps/web/src/services/api/authFiles.ts) 参考管理列表/状态和按 auth-file 获取模型的调用形状；管理操作有范围检查，不能把非秘密显示信息直接作为调用身份。
- [OAuth API](https://github.com/seakee/CPA-Manager-Plus/blob/05ebb7f275dbe575211cb886436d4b99936c1cd9/apps/web/src/services/api/oauth.ts) 和 [AuthFileType](https://github.com/seakee/CPA-Manager-Plus/blob/05ebb7f275dbe575211cb886436d4b99936c1cd9/apps/web/src/types/authFile.ts) 的 provider 集合不同。provider 类型并非全 OAuth；CPA 的 [VertexCredentialStorage](https://github.com/router-for-me/CLIProxyAPI/blob/e2bff0107bb307337aaa19018ccddd55f64253d5/internal/auth/vertex/vertex_credentials.go) 持有服务账号 JSON，配置另有 Vertex API Key 入口。
- 参考连接状态、错误与缺字段的表达；不引入 CPAMP 的 usage SQLite/采集、dashboard 或全套 provider 管理。套餐数据未知保持未知，不能从上述类型表推导真实用户的可用模型。

## 未验事项与停止边界

实际 CPA artifact 与普通 HTTP 定向由 R1 验证；订阅流程/存储和身份状态由 R3 的替身闭环及 R7 真人验证；DSH 实际协议和配置口径由 R5 核对，真实链路由 R7 验收。当前不要求 R1 登录或生成真实内容。真实套餐权益、可靠额度、禁止额外消费、各模型协议能力及 DSH 真实调用均未成立；它们不能因本次选型文档发布而改成通过。
