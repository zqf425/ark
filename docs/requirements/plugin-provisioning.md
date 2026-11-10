# 插件 provision 启用流（agent-desktop 模式）

## 需求与范围

目录插件可以声明一个 `provision` 段，把「外部工具 + 外部 MCP server」作为插件接入。启用流程由
**宿主**执行，插件只贡献声明、不贡献代码：

1. 用户点击「启用插件」。
2. 宿主运行 `detect` 命令检测本机是否已安装该工具。
3. 已安装 → 宿主把 `mcp_server` 声明注册进外部 MCP server 注册表（`McpServerService::upsert`），
   其工具以 `<server_id>.<tool>` 并入 `mcp` 工具组，插件状态变为 `enabled`。
4. 未安装且用户未同意安装 → 状态 `needs_install`，提示用户；插件保持**不起用**。
5. 用户同意安装（`POST enable { "install": true }`）→ 宿主运行 `install` 命令；成功后重新检测并
   注册 MCP；失败 → 状态 `failed`，插件仍不起用。
6. 状态持久化在 `<plugins_root>/<plugin_id>/provision-state.json`；启动时若持久化为 `enabled`
   会重跑 `detect`，二进制消失则降级为 `needs_install`（MCP 条目本身由注册表持久化，重启自动重连）。

## HTTP 面

- `GET /v1/runtime/plugins/{id}/provision` → `PluginProvisionView`
- `POST /v1/runtime/plugins/{id}/enable`，body `PluginEnableRequest { install: bool }` →
  `PluginProvisionView`
- `GET /v1/runtime/plugins` 的 `PluginEntry` 携带可选 `provision` 字段，设置页据此渲染状态与按钮。

## 治理规则（fail closed）

- `detect.command` / `install.command` / `mcp_server.command` 必须是**裸二进制名**（仅
  `[A-Za-z0-9._-]`，无路径、无 shell、无占位符展开）；参数是静态字符串数组。
- `install.command` 额外受安装器白名单限制：`npm | pnpm | yarn | brew | cargo`。安装是唯一可能
  执行任意 setup 代码的步骤，因此被收窄到允许列表，且必须来自用户显式同意（`install: true`）。
- 子进程：清空环境仅保留 `PATH`/`HOME`、无 shell、输出封顶（detail 截断）、超时
  （detect 10s / install 600s，超时杀进程）、通过 `tokio::task::spawn_blocking` 执行。
- 校验在装载期完成（`ProvisionSection::validate`）；失败即 `plugin_manifest_invalid`，
  插件被剔除，不影响其他插件。
- 未启动（`failed`/`skipped`）的插件不生成 provision spec，其声明保持惰性。
- `mcp_server` 仅支持 stdio 传输（命令 + 参数），`allow_tools` 透传给注册表。

## 状态机

`disabled → (enable) → detect ─┬─ ok → upsert mcp ─→ enabled`
`                             └─ fail ─┬─ 未同意 → needs_install`
`                                     └─ 同意 → install ─┬─ ok → detect → upsert → enabled`
`                                                       └─ fail → failed`

`failed` / `needs_install` 均为「不起用」：插件不注册 MCP、不产生工具。

## 示例：agent-desktop 插件

`<data_dir>/plugins/ark.plugins.agent-desktop/plugin.json`：

```json
{
  "id": "ark.plugins.agent-desktop",
  "version": "1.0.0",
  "kind": "tool",
  "capabilities": ["tool"],
  "provision": {
    "detect": { "command": "agent-desktop", "args": ["status"] },
    "install": { "command": "npm", "args": ["install", "-g", "agent-desktop"] },
    "mcp_server": {
      "id": "agent-desktop",
      "command": "agent-desktop",
      "args": ["mcp"],
      "allow_tools": ["snapshot", "click", "set-value", "screenshot", "type", "key"]
    }
  }
}
```

`mcp_server.id` 即外部 MCP server 注册表条目 id；其启动子命令以 `agent-desktop --help` 实际
输出为准（v1.2.0 起官方自带 MCP 支持）。

## 测试

- `ark-plugin`：manifest 校验（路径/白名单/空 id 拒绝）、启用流（检测通过直接注册、未同意停在
  `needs_install`、同意后安装再注册、安装失败 `failed`、状态持久化与启动降级、未知插件报错）。
- `ark-server` / `arkd`：路由与装配回归（180 / 99 项测试通过）。
- web：`tsc --noEmit`、settings 相关 vitest、硬约束检查。
