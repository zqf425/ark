# ark-contract 模块设计

## 职责与边界

`ark-contract` 是 HTTP、SSE、MCP 与 React 渲染层之间的 wire DTO 唯一真源。它依赖
`ark-common`，但不得包含端口 trait、持久化实体、内部查询对象或业务实现。

当前公开面包括线程/回合/子代理 DTO、运行时事件、渲染 item、能力目录、配置视图、provider 与
skill 视图。所有 GUI 字段使用 camelCase；既有查询参数保持既有命名，例如 `since_seq`。

## 类型生成

- Rust 类型派生 `serde`、`schemars` 与 `ts-rs`；前端只导入 `web/src/generated/` 生成物。
- 修改 wire 类型后运行 `cargo test -p ark-contract --lib ts_export`，再检查
  `git diff --exit-code web/src/generated`。
- 新增根类型必须同时登记 `ts_export.rs` 的 `export_roots()` 与 `ROOT_NAMES`。

## 注意事项

错误码的通用定义当前位于 `ark-common::ArkErrorCode`，而非本 crate；这是当前实现的明确例外。
若未来迁移错误码，必须一次性更新 HTTP、SSE、MCP 和生成的 TypeScript，不能保留双源。
