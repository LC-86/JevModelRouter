# 用独立本地服务复用模型接入能力

用户于 2026-10-02 将首个产品目标确定为 Mac 本地模型中转站：多类来源汇成模型池、手选模型及来源供 DSH 调用，智能路由后置。新阶段优先采用 CLIProxyAPI 独立服务复用已支持来源的认证、刷新、上游访问及协议转换；Jev 拥有连接引用、身份/世代、稳定模型目录、手选目标、准入和自有服务生命周期。相比继续逐家构建官方 CLI 代理，这减少重复认证和协议维护；相比复制 CPA/CPAMP 源码，独立边界保留上游升级与测试责任。CPAMP 只作为管理接口/状态参考。

这替换 [#6](https://github.com/LC-86/JevModelRouter/issues/6) 与 [旧规格 #10](https://github.com/LC-86/JevModelRouter/issues/10) 的“官方 Codex App Server/Grok CLI 作为首选”路线，保留原生权益、凭据隔离、客户端工具控制和 [#9](https://github.com/LC-86/JevModelRouter/issues/9) 的仅订阅内消费约束；旧 CLI 实现和证据仍保留，真实能力未启用。现行行为和实施范围只由 [#51](https://github.com/LC-86/JevModelRouter/issues/51) 定义。

采用方向不代表接入验收完成。[R1 / #52](https://github.com/LC-86/JevModelRouter/issues/52) 必须以实际 CPA + 假上游证明普通模型 HTTP 可固定来源/账号且失败不串用；优先唯一 prefix/单凭据 namespace，不能成立时验证独立单凭据实例。内部 SDK 的 pinned auth 不冒充公共 HTTP 契约。接口、许可、维护边界与未验项见[固定源码核对](../research/local-relay-reuse.md)。真实账号、费用边界和 DSH 实机可用性由 [R7 / #58](https://github.com/LC-86/JevModelRouter/issues/58) 建立。
