# ark-http 模块设计

## 职责与边界

`ark-http` 是唯一的出站 HTTP 基础能力，实现 `HttpCapability`。插件和 provider 不得自行创建
未受治理的客户端；所有外部请求经本模块应用 allowlist、凭据注入、超时、重试、限流、脱敏与追踪。

## 安全与隔离

- 请求按 `plugin_id + host` 校验 manifest allowlist、限流与重试；熔断属计划能力，当前未实现，
  不得宣称已交付。
- `Authorization`、`Host` 与受保护 User-Agent 只能由宿主设置，插件仅引用 `CredentialRef`，不能读取
  明文。trace 头的**值**允许插件合法 pass-through（`accept_or_mint` 采纳有效 `x-ark-trace-id`），
  但线上写入该头只由宿主完成。
- 重定向必须逐跳重新校验目标；日志只保留脱敏 URL、凭据标签和哈希前缀。
- 不同插件的请求身份、配额与诊断相互隔离。
- 不受限制的 allowlist 只能经 `HostAllowlist::allow_all` +
  `EgressRegistry::register_grant` 由组合根为运行时自有出口（`ark.builtin.web`）构造；
  `parse` 永远拒绝裸 `*`，因此插件清单不可能声明全放行。
- 运行时自有出口的 SSRF 防线（拒绝私网/loopback 目标）由调用方工具层实现，本模块保持
  字符串 allowlist 语义不变。

## 验证

覆盖 allowlist/通配符、保留头覆盖、重定向越界、凭据归属、重试幂等性、限流与敏感字段脱敏。
