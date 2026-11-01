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

## 生命周期

启动前先解析 `requires`、版本范围、循环和重复 ID；声明错误阻止启动。单个插件 `register` 失败时，
只跳过它及依赖方并记录诊断，健康插件继续运行。`Registration` 的资源必须在 drop 时撤销。

## 隔离规则

`PluginIdentity` 只能由宿主构造。配置、KV、文件、凭据、日志、追踪、指标、通知和 HTTP/MCP/UI
句柄均已绑定 `plugin_id`，不暴露 owner 参数，也没有关闭隔离的 manifest 开关。

| 服务句柄 | 插件可做什么 | 宿主强制的边界 |
|---|---|---|
| HTTP | 向声明的域名请求 | allowlist、受保护请求头、限流、凭据调用期注入 |
| MCP | 发布/调用声明的工具 | 命名空间、可见性、调用深度和循环检测 |
| UI | 注册页面和面板 | route 冲突、能力门禁、主题和鉴权 |
| config/KV/files | 维护私有配置和状态 | `plugin_id` 命名空间、相对路径、配额 |
| credentials/log/trace | 请求凭据、记录诊断 | 不返回明文，身份/脱敏字段由宿主注入 |

manifest 必须描述稳定 ID、版本、kind、能力、依赖和配置 schema。声明与实际注册项不一致、循环依赖、
缺失服务、主版本不兼容或重复 ID 都应在调用插件代码前报错。

## 计划边界

平台服务的完整发布与外部插件运行时仍是目标态。文档中提及的未来插件目录、市场或 WASM 不得被
当前实现依赖；新增服务前必须先定义端口、授权和失败隔离。
