# 插件宿主接线（ark-plugin / ark-http 接入 arkd）

> 状态：`ark-plugin` 与 `ark-http` 已实现但未接入组合根，全仓无任何宿主装配。本文档为接线需求
> 设计提案；批准前不实施，批准后其"计划"节即构成已批准计划。

## 当前实现

- `crates/ark-plugin` 已具备完整领域与服务层：manifest 校验（稳定 ID、版本、kind、能力、依赖、
  配置 schema）、依赖拓扑 `resolve_order`/`plan_skips`、宿主私有构造的 `PluginIdentity`
  （`domain/identity.rs` 的 `pub(crate) host_issue`）、`PluginContext` 及全部绑定 `plugin_id` 的
  隔离句柄（config/KV/files/credentials/log/trace/metric/notify 与 HTTP/MCP/UI 句柄，
  `service/context.rs` + `service/isolation.rs`）、`PluginRegistry.load/unload`
  （`service/registry.rs`，含 `LoadReport`/`PluginDiagnostic`）、`Registration` drop 反注册。
- `crates/ark-http` 的 `HttpBus`（`service/outbound.rs`）已实现并实现
  `ark_ports::HttpCapability`，当前仅被 `ark-mcp` 的 egress 引用。
- `crates/ark-mcp` 已有插件工具出口：`McpBus` 的 `expose_with_handlers`/`expose_specs`，插件工具
  名恒为 `<plugin_id>.<tool>`，且已有 `McpVisibility` 可见性目录、`authorize` 与调用深度限制。
- `apps/arkd` 组合根未依赖这两个 crate；全仓 `ark_plugin::` 零命中。插件能力端口
  （`ark-ports/src/services.rs`、`bus.rs`）已按"宿主绑定 plugin_id"设计就绪。

结论：缺口只在组合根装配与插件来源，不在能力层。

## 计划（本设计批准后生效）

### 目标与非目标

目标：`arkd` 启动时从本地插件目录装载插件，使其工具、UI 面板与受治理 HTTP 能力经既有总线可用；
单个插件失败只跳过自身及依赖方，不阻断启动。

非目标：插件市场、远程分发、WASM 运行时、热重载、web 端插件管理界面（均见"待决"）。

### 组合根装配

- `apps/arkd/Cargo.toml` 增加 `ark-plugin`、`ark-http` 依赖。
- 装配点：`apps/arkd/src/wiring.rs` 平台服务段之后、工具组替换
  （`tools.replace_group("mcp", ...)`，约 `wiring.rs:211-213`）之前构造 `HttpBus(policy, vault,
  sink)` 与 `PluginRegistry`，在 UI bus 段（约 `wiring.rs:118-135`）之后完成插件加载。
- 插件来源：首版仅支持 `<data_dir>/plugins/<plugin_id>/` 本地目录 + manifest（沿用
  `ark-plugin` domain 清单格式），不引入网络分发与信任签名。
- 启动顺序：扫描目录 → manifest 校验与拓扑解析（循环/重复 ID 在调用插件代码前报错）→ 逐个
  `load`（携带诊断 sink）→ `LoadReport` 写入启动日志与诊断接口。

### 安全与隔离（继承 ark-plugin.md，不新增豁免）

- HTTP 出站受 `HttpBus` allowlist、受保护请求头与限流约束；凭据仅调用期注入，任何接口不返回明文。
- 插件工具经 `McpBus` 命名空间暴露为 `<plugin_id>.<tool>`，跨插件调用受可见性与深度限制约束。
- `Registration` drop 后其工具、页面、句柄必须不可再被解析。
- manifest 声明与实际注册项不一致、缺失服务、主版本不兼容均在注册前报错并跳过该插件。

### 验收场景

1. 最小插件（manifest + 一个 MCP 工具 + 一个面板）在真实 `arkd` 启动后端到端可见可用。
2. 故障插件（坏 manifest / register 失败 / 循环依赖）被跳过并产出诊断，健康插件正常运行。
3. 插件反注册后其工具与面板不可再解析；无孤儿注册残留。
4. 未授权域名的插件 HTTP 请求被拒绝且诊断可查。
5. `cargo test --workspace --exclude ark-desktop` 通过；涉及组合根改动覆盖故障路径回归。

## 待决

- 插件工具并入现有 `mcp` 工具组还是新增独立 `plugin` 组（影响 web 目录展示与模型可见工具名）。
- 插件启停配置（config 中 per-plugin enable/disable 与配置 schema 实例化）及诊断在 web 的展示方式。
- 热重载与运行时卸载语义；本地插件的信任模型（是否需要显式用户启用确认）；WASM 时机。
