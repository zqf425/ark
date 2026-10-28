# web 渲染层设计

## 职责与边界

`web/` 是 React + TypeScript Code 模式渲染层。它使用 `ark-contract` 生成类型和 typed client 渲染线程、
回合、工具、审批与设置；业务语义、路径归一化、脱敏、截断和事件到 item 的映射不应在前端重复实现。

## 约束

- 不手写 Rust 已拥有的 DTO、错误码 union 或类型别名；未建模的请求组合和纯 UI 状态仅放在
  `src/shared/contracts/web-only.ts`。
- 运行时请求集中在 feature API/client 层；组件不直接裸 `fetch('/v1/...')`。
- SSE 消费保存最后 `seq`，断线后使用 `since_seq` 恢复；页面必须能处理压缩产生的显式空洞。
- `child_completed` 按稳定 `notification_id` 去重后显示持久摘要卡片；实时到达时显示一次性提示。摘要仅作
  父线程状态展示，不能在 web 侧重建或改写模型上下文。
- `child_completions` 详情与 reducer 内存均限制为最新 50 条；完成、失败和中止状态必须本地化为用户文案，
  不直接显示内部枚举值。
- Code 右侧的 Plan、Changes、File、Terminal 均调用服务端投影；File 仅预览本线程已成功 `read` 的工具
  输出，浏览器不得直接读取工作区。子代理控制请求始终带父线程 query，不能只凭 child id 操作。
- 设置页的 tuning、provider 与 MCP 都使用生成契约；API key、MCP env/header 值只写不回读，编辑时的空
  secret 字段表示保留，不表示删除。
- 组件状态按 feature 拆分，避免重建旧 `Workbench.tsx` 巨型组件；UI 变更应进行实际浏览器验证。

## 计划边界

Tauri 外壳和独立 `plugin-code-ui` 尚未成为当前 workspace 模块。实现相关需求时先更新本文件和需求设计，
再决定其与 `ark-ui` 内置注册的迁移方案。
