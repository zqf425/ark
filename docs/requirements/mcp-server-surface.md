# ark 原子能力 MCP 发现接口设计

> 状态：**当前实现（一期）与已批准计划**。本文定义 ark 对外暴露"可被其它 agent 发现并调用的
> 原子能力"的方式：一个自描述的 `GET /v1/mcp-info` 元数据接口，返回全部工具（名称、说明、
> 参数 schema、对应的 HTTP 方法与路径）与鉴权说明；调用方据此直接调用既有 REST API。
> 早期 v0.4 方案（"把线程、回合、审批、用量、Studio 暴露为 MCP 工具"）中未交付的部分，
> 以本文的现状与计划标注为准。

## 模式：描述式发现，而非协议服务端

参照同类实现（`/api/mcp/server-info` 模式），ark **不**为本能力新启动服务、不实现 MCP JSON-RPC
服务端：工具的执行就是既有 REST 端点本身，元数据接口只回答"有哪些能力、怎么调、怎么鉴权"。
这样工具目录与路由表永远指向同一组端点，不存在第二套执行通道，也不扩大攻击面。

## 端点与鉴权

- `GET /v1/mcp-info`，挂在免鉴权路由组（与 `/health` 同层）：发现先于鉴权是刻意设计，调用方
  需要先读本文档才知道如何取得并携带 token。文档本身不含任何敏感数据。
- 响应结构：

```json
{
  "server_name": "arkd",
  "version": "<构建版本>",
  "transport": "http",
  "auth": {
    "required": "<运行时是否配置了 auth_token>",
    "header": "authorization",
    "description": "把 arkd 启动参数 --auth-token 的值以 `authorization: Bearer <token>` 头随每个 /v1 请求携带（/health 与 /v1/mcp-info 除外）。required 为 false 表示运行时未配置 token（本地显式放开，不是默认安全）。"
  },
  "tools": [ { "name": "...", "description": "...", "inputSchema": {}, "http": { "method": "POST", "path": "/v1/threads" } } ]
}
```

不包含 `resources`/`prompts` 等 MCP 协议工件字段：本文档是纯 HTTP 发现接口，不承载协议握手。

- 每个工具带 `http.method` + `http.path`（路径参数以 `{id}` 等占位），调用方替换占位后直接请求。
  成功状态码遵循既有 REST 语义（创建 201、启动回合 202、普通读取 200、无内容删除 204）。
- 错误结构与 REST 一致（`code`/`message`/`detail`），不因从元数据入口发现而变化。

## 工具目录

工具名与 REST 路由一一对应（`<资源>_<动作>` 蛇形命名，ASCII），注册表是 `ark-server` 内的静态表，
是路由之外**唯一**的工具目录真源；新增路由时同步登记。`ark-contract` 是 wire 字段的唯一真源，
因此 inputSchema 保持精简：路径参数与调用方必须写对的少量字段逐项说明；由契约 DTO 建模的请求体
只引用类型名（如 `TurnRequest`，生成物见 `web/src/generated`），**不**逐字段抄写——照抄字段就是
第二份会漂移的清单。`ark-server` 新增对集成测试的路由一致性探测（见下），保证目录与路由表一致。

一期目录覆盖全部已落地路由族：

- 线程族：list/create/get/update/delete/fork/rewind/compact/export/review/events(SSE)/usage/skills；
- 回合族：start/get/steer/interrupt，以及审批 `approvals_resolve`、用户输入 `user_inputs_resolve`；
- workbench：plan/changes/files/terminal/goal/todos 读写；
- 定时任务：list/create/update/delete/run；
- memory、attachments（上传/读取/删除/内容/诊断/线程关联）；
- child-runs 管理族（list/get/delete/abort/steer/resume/thread/events）；
- 平台管理面（providers、skills、workspace、mcp servers、plugins、runtime info/tools/capabilities、
  config、subagents、sessions resume、debug llm-rounds、shutdown）。

## 非目标（一期不做，后续需求另立）

- MCP JSON-RPC 协议服务端（stdio 子进程或 streamable-HTTP 的 initialize/tools/list 握手）：
  若未来某类客户端只能走协议而非 REST，再立需求设计；
- `tools/run` 式统一执行器：ark 的 REST 已是原子接口，增加执行器只会复制鉴权与错误语义；
- 工具级 grant/审批作用域：启动回合、审批 resolve 等敏感能力继续受既有 gate 与 token 模型约束，
  不在元数据层新增权限模型。

## 验收与验证

- 注册表单测：工具名唯一、`http.path` 均以 `/v1/` 开头、inputSchema 均为 object 且含
  `properties`、method 属于 GET/POST/PATCH/DELETE。
- 路由一致性测试：对注册表中每个 `method+path`（占位符替换为格式合法的哑 id）发起真实请求，
  断言响应**不是**路由级 404（axum 未匹配返回空体 404；handler 级 404 返回结构化 JSON），
  防止目录与路由表漂移。
- handler 测试：`/v1/mcp-info` 返回 200、`auth.required` 随 token 配置变化、`version` 为构建版本。
