# ark-memory-plugin

记忆插件。本文档记录该模块的**当前实现**、已批准计划与待决边界；目标态不写成既成事实。

## 1. 定位与约束

记忆插件是一个**独立 workspace crate**（`crates/ark-memory-plugin`），以目录插件
（directory plugin）形态分发，不进默认打包。

红线（来自整体架构 §3 与设计 §7.7）：

* **不依赖 `apps/arkd`**：crate 依赖只有 `ark-common` / `ark-ports` / `ark-plugin`
  以及序列化与时间库。它不 import `arkd` 的任何模块。
* **不依赖 `ark-agent` / `ark-store` / `ark-llm`**：记忆插件不参与模型决策循环，
  不直接读写会话存储，不调用模型。
* **只用平台开放能力**：`PluginContext` 暴露的句柄（`kv` / `files` / `cfg` /
  `http` / `log` / `notify` / `metrics`）与 `expose_tool_handlers`。
* **默认不集成**：根 `Cargo.toml` 的 `members` 收录本 crate（保证它能被编译和测试），
  但 `arkd` 的启动路径不引用它；用户从插件界面安装后才生效。

## 2. 插件能力边界（当前实现）

设计 §7.7 的目录插件是**声明式**的：manifest 里的 `tools[].action` 只有两种
（`ark-plugin/src/domain/directory.rs:176`）：

| action | 语义 | 记忆插件可用性 |
| --- | --- | --- |
| `static` | 原样返回声明好的 payload | 只读、无状态 |
| `http` | 经 `HttpCapability` 发一次受限外呼 | 需要外部记忆服务 |

**纯声明式 action 无法表达"存到本地、按关键词检索"这类有状态逻辑。**
因此记忆插件的本体不写成 `action`，而是：

* 用 `expose_tool_handlers` 注册**带 handler 的进程内工具**（`ark-mcp` 的
  `McpHandler`，真实实现，不是 `expose_specs` 的 `Unavailable` 占位）；
* 用 `PluginContext.kv` / `.files` 落盘，二者主机侧已按 plugin id 物理隔离。

这条路径的宿主是"进程内装载本 crate 的 arkd"，与"界面安装二进制"是两条不同的
分发路线，见 §5 的待决项。

## 3. 存储模型（当前实现）

三级记忆，全部落在插件自己作用域内：

| 层 | 载体 | 键/路径 | 用途 |
| --- | --- | --- | --- |
| 短期 | `ScopedKv` | `<plugin_id>::m:<id>` | 当轮快速读写 |
| 长期 | `ScopedFiles` | `data/plugins/ark.memory.core/memories/<id>.json` | 持久化、可人工检查 |
| 索引 | `ScopedKv` | `<plugin_id>::idx` | 关键词倒排，避免全量扫描 |

`ScopedKv` 是同步且内存驻留的（`isolation.rs:131`），`ScopedFiles` 是唯一跨重启载体，
所以**写路径先落文件、后更新 KV 索引**；启动时以文件为准重建索引。这与
`ark-store` 的"日志是唯一真源、索引可重建"规则一致。

记忆条目（wire 形状在插件内部定义，不进 `ark-contract`，因为它不跨进程）：

```json
{
  "id": "mem_<uuid>",
  "text": "...",
  "tags": ["preference"],
  "source": "manual|turn",
  "created_at": "2026-11-19T09:00:00Z"
}
```

## 4. 工具面（当前实现）

以 `<plugin_id>.<tool>` 发布：

| 工具 | 入参 | 行为 |
| --- | --- | --- |
| `memory_store` | `text`, `tags[]`, `source?` | 写入一条记忆，返回 id |
| `memory_search` | `query`, `tags[]?`, `limit?` | 关键词 + 标签过滤检索 |
| `memory_list` | `limit?` | 按时间倒序列出 |
| `memory_delete` | `id` | 删除一条（KV + 文件 + 索引） |

检索刻意只做**关键词 + 标签**，不做向量检索：嵌入模型属于 `ark-llm`，插件无权访问，
用 HTTP 外呼嵌入会让"本地记忆"变成需要联网的功能。向量检索记为待决（§5）。

## 5. 与 arkd 现有能力的关系和缺口

### 5.1 已具备

* `PluginContext::expose_tool_handlers` 能注册真实现工具（`ark-mcp` 提供）。
* `ScopedKv` / `ScopedFiles` 提供按 plugin id 隔离的存储与目录包含校验。
* `ScopedConfig` 提供按插件分键的配置。
* `POST /v1/runtime/plugins/{id}/enable` + `provision` 段提供"检测 → 用户确认 →
  安装 → 注册 MCP server"的受控启用流程，安装命令限定在
  `npm|pnpm|yarn|brew|cargo` 白名单（`directory.rs:276`）。
* `GET /v1/runtime/plugins` 已能报告每个插件的 pages/panels/tools 与诊断。

### 5.2 缺口（本插件遇到的问题，需要宿主侧决策）

1. **无动态装载入口。** `PluginHost` 只有 `unload()`，没有 "install from path /
   reload" 路由；插件在 `load_host` 时一次性读盘。所以"从界面安装一个记忆插件后
   立即生效"当前做不到，只能重启 arkd。
2. **进程内 handler 与目录插件是两条路。** `expose_tool_handlers` 是 Rust API，
   只有随 arkd 一起编译进去的 crate 才拿得到 `PluginContext`；磁盘上的
   `plugin.json` 只能拿到声明式 action。二者没有桥。
3. **`provision.install` 只有命令白名单，没有来源治理。** `cargo` 在白名单里，
   但 manifest 只能给 `command` + 静态 `args`，没有版本/源约束，也没有摘要校验。
   用它安装一个二进制属于**执行第三方代码**，需要单独的信任设计，本次不启用。
4. **`ScopedKv` 无持久化。** 它是 `BTreeMap`，进程退出即丢。所以本插件把文件当
   唯一真源，KV 只作加速；若宿主未来给 `ScopedKv` 加持久化，这一层可以简化。
5. **无事件订阅。** `HostHandles.events` 在 `arkd` 装配时是 `None`
   （`apps/arkd/src/plugin_host.rs:150`），插件无法监听回合结束来自动沉淀记忆，
   因此 `source: "turn"` 目前只能由模型显式调用 `memory_store` 产生。

## 6. 已批准计划

* 本次只交付 §3、§4 描述的能力，形态为**随 arkd 编译的进程内插件**，
  由 `arkd` 组合根在插件装配阶段注册；manifest 只用于报告与界面展示。
* 插件管理界面复用 `settings -> Plugins` 已有的只读诊断面板，不新增页面，
  不引入新的前端依赖。

## 6.1 本次实际交付范围

| 项 | 状态 |
| --- | --- |
| `crates/ark-memory-plugin`（4 个工具 + 20 个测试） | 已实现并通过 |
| `docs/modules/ark-memory-plugin.md` | 已实现 |
| `settings -> Plugins` 增加「安装插件」说明区（i18n 中英文） | 已实现 |
| 从界面**真正安装**第三方插件 | **未实现**，被 §5.2 的缺口阻塞 |
| 记忆插件接入 `arkd` 默认启动流程 | **未实现**，需先定 §7 的第 1 项 |

关于「从界面安装」的现状必须明确：本版本没有动态装载入口，界面能做的只是
把安装方式讲清楚（插件目录、`plugin.json` 最低要求、需重启）。真正的
一键安装需要一个新增的宿主能力，见 §7。

## 7. 待决

* **记忆插件是否接入默认启动流程。** 当前 crate 只编译不装配：`arkd` 启动路径
  没有引用它，所以安装形态是「把目录放进插件根」还是「由组合根编译期决定」
  尚未定。选前者需要 §7 的下一项。
* **是否需要真正的「界面一键安装」。** 需要宿主新增动态装载（install from
  path + 重新 `load`）与来源签名校验，属独立需求
  （`docs/requirements/plugin-installation.md` 尚未创建）。
* 记忆插件是否要支持向量检索（需要宿主开放嵌入能力或允许 `ark-llm` 外呼）。
* 是否需要「回合结束自动沉淀记忆」（需要宿主开放事件或钩子）。
* **`FileService` 缺少删除与目录枚举。** 记忆插件为此改用墓碑标记 + 自维护
  `memories.index` 清单文件。若宿主补上 `remove_relative` 与
  `list_relative`，插件可去掉这两处绕行。