# ark-plugin 模块设计

## 职责与边界

`ark-plugin` 负责 manifest 校验、依赖拓扑、宿主签发的插件身份、隔离句柄、注册与撤销。它不承载
具体业务插件的领域逻辑。

## 插件来源：本地目录

首版宿主只装载本地目录插件，不引入网络分发、签名或 WASM 运行时。每个插件占一个目录
`<plugins_root>/<plugin_id>/plugin.json`：

- `scan` 读取并校验每个文件（`service/directory.rs`），坏文件产生 `plugin_manifest_invalid` 诊断，
  从不阻断启动。
- `service/directory.rs` 的 `DirectoryPlugin` 实现 `Plugin`：它把声明式贡献（`ui.pages`、`ui.panels`、
  `tools`）在 `register` 中落到既有句柄，因此目录插件不能比编译期插件更宽松。
- `ToolAction` 只允许两种宿主提供的动作：`static`（原样返回声明载荷）与受治理的 `http`（一次
  `HttpCapability` 请求）。插件因此永远拿不到执行任意代码的入口。

`service/host.rs` 的 `load_host` 是唯一把目录变成运行中插件集合的地方，顺序固定为
“扫描 → 拓扑规划（`plan_local_load`）→ 逐个 `PluginRegistry::load`”，并在任何插件代码运行前声明
出站名单、失败后立即撤销（fail closed）。

## 工具向模型暴露

插件工具经 `ark-mcp` 的 `McpBus` 发布为 `<plugin_id>.<tool>`；`apps/arkd` 用
`ark_mcp::PluginTool` 把它们并入现有 `mcp` 工具组，不新增独立分组。宿主入口走
`McpBus::dispatch_from_host`：它跳过插件间 `mcp.consume` 门禁（用户不是插件），但保留命名空间、
调用深度、循环与死锁守卫。`name`/`schema` 在构造时 leak 一次，保证稳定前缀字节稳定。

## 诊断与只读报告

`PluginHost::host_report` 产出 `ark_contract::PluginHostReport`（已装载插件、`failed`/`skipped`、
启动诊断、拒绝的 `http_denials`），由 `GET /v1/runtime/plugins` 只读暴露，经 `ark_ports::
PluginHostService` 注入 `BaseServices`。组合根不重新解释这些字段。

## Provision 启用流（已落地）

目录插件可声明 `provision` 段（`detect`/`install`/`mcp_server`），把「外部工具 + 外部 MCP server」
接入本运行时；需求设计见 [`docs/requirements/plugin-provisioning.md`](../requirements/plugin-provisioning.md)。
启用流程由宿主执行，插件只贡献声明、不贡献代码：

- `POST /v1/runtime/plugins/{id}/enable` 触发 `service/provision.rs` 的状态机：detect → 同意后
  install → `McpServerService::upsert` 注册 stdio server（工具以 `<server_id>.<tool>` 并入
  `mcp` 工具组）；未安装且未同意停在 `needs_install`，安装或注册失败停在 `failed` —— 均为
  不起用状态。状态持久化在 `<plugins_root>/<plugin_id>/provision-state.json`，启动时重跑
  detect，二进制消失则降级为 `needs_install`。
- 治理：命令必须是裸二进制名（无路径、无 shell、静态参数）；`install.command` 受安装器白名单
  （`npm|pnpm|yarn|brew|cargo`）限制且必须来自用户显式同意；子进程清空环境、输出封顶、超时
  杀进程。校验在装载期 fail closed，未启动的插件不生成 provision spec。
- 组合根仅机械装配：`wiring` 把 `McpServerAdapter` 同时交给 `PluginHost::attach_mcp_servers`；
  `PluginHostService` 新增 `provision_status`/`enable` 两个入口。

## 生命周期

目录装载的失败隔离策略是“剔除 + 诊断 + 继续启动”：`plan_local_load` 把缺依赖、版本不兼容或成环的
插件剔除并记录诊断，健康插件照常装载——一个坏声明不得阻止整个运行时（目录名 == manifest id，
结构上不会出现本地重复 ID；重复 ID 检查属于编译期插件路径）。单个插件 `register` 失败时，
只跳过它及依赖方并记录诊断。`Registration` 的资源必须在 drop 时撤销。

声明与实际注册项不一致、循环依赖、缺失服务、主版本不兼容在调用插件代码**之前**报错（剔除该插件）。

## 隔离规则

`PluginIdentity` 只能由宿主构造。配置、KV、文件、凭据、日志、追踪、指标、通知和 HTTP/MCP/UI
句柄均已绑定 `plugin_id`，不暴露 owner 参数，也没有关闭隔离的 manifest 开关。

| 服务句柄 | 插件可做什么 | 宿主强制的边界 |
|---|---|---|
| HTTP | 向声明的域名请求 | allowlist、受保护请求头、限流、凭据调用期注入 |
| MCP | 发布/调用声明的工具 | 命名空间、可见性、调用深度和循环检测 |
| UI | 注册页面和面板 | route 冲突、能力门禁、主题和鉴权 |
| config/KV/files | 维护私有配置和状态 | `plugin_id` 命名空间、相对路径（配额为计划，未实现） |
| credentials/log/trace | 请求凭据、记录诊断 | 不返回明文，身份/脱敏字段由宿主注入 |

manifest 必须描述稳定 ID、版本、kind、能力、依赖和配置 schema。声明与实际注册项不一致、循环依赖、
缺失服务、主版本不兼容或重复 ID 都应在调用插件代码前报错。

## 计划边界

平台服务的完整发布与外部插件运行时仍是目标态。文档中提及的未来插件目录、市场或 WASM 不得被
当前实现依赖；新增服务前必须先定义端口、授权和失败隔离。

## 已批准计划：事件钩子总线

需求设计见 [`docs/requirements/plugin-hook-bus.md`](../requirements/plugin-hook-bus.md)。
本模块将新增（尚未实施，属已批准计划）：

- **钩子授权**：manifest 新增独立 `hooks` 段（`#[serde(default)]`）与 `hook_abi` 版本声明，逐点
  授权；不扩 `Capability` 枚举。钩子句柄进 `Registration`，Drop 即反注册；句柄即授权
  （`ctx.hooks()` 无授权点则无对应方法）。`hook_abi` 不匹配在扫描/计划阶段剔除，复用
  `plugin_skipped` 诊断。
- **钩子四级**：Observe（可丢 + 水位重放）/ Decide（同步窄改写、硬预算、熔断即撤句柄）/
  Supply（prompt sections、tool catalog filter）。v1 的 Decide/Supply 仅 `ark.builtin.*` 内置
  插件可用；目录插件（声明式）不声明钩子。
- **Observe 投递**：不开放裸 `EventSink`；宿主为每个插件懒启动 per-thread 订阅任务（既有
  `subscribe(thread, since_seq)` 语义），有界、按插件隔离，插件持水位续传。
- **审计**：新增 `HookAuditSink`（照 `HttpDiagnosticSink`/`PluginDiagnosticView` 形状：有界环 +
  wire 视图），记录钩子点、决策、延迟与 diff；`PluginHostReport` 相应扩展可选字段。
- **回合计入与定时**：followup 经 `CompletionInbox` plugin 来源条目（空闲优先，不抢占）；
  `on_tick` 为插件可选生命周期回调。二者与配额、`TurnSource::Plugin` 归因同批落地（P3）。
