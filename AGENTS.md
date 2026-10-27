# ark 开发规范（AGENTS.md）

本文只保留所有改动都必须遵守的流程和跨模块红线。整体架构以
[`docs/rust-rewrite-design.md`](docs/rust-rewrite-design.md) 为准；模块内部的详细设计以
[`docs/modules/`](docs/modules/README.md) 为准。二者冲突时修正文档，不靠口头约定。

## 1. 开发流程

1. **代码修改必须使用独立 worktree**：从主分支创建 worktree，在其中实现、验证并检查 diff，
   确认后才合入主分支；并行任务不得共用 worktree。
2. 新增功能前先检查同模块是否已有相同或相近能力；能扩展或复用时不得重复实现。
3. 修改范围保持最小：只改完成当前任务必需的代码与引用，不顺手重构或改变无关行为。
4. 修改前必须阅读：
   - 整体架构文档；
   - 全部受影响模块的设计文档；
   - 变更跨越端口、契约或调用边界时，相关相邻模块的设计文档；
   - 对外行为、数据迁移、状态机、权限、配置或多模块协作的对应需求设计。
5. 修改后同步更新受影响模块设计；改变需求边界或验收标准时同步更新需求设计。新模块先建立模块
   设计文档，再写实现。

## 2. 文档治理

```text
docs/
├── rust-rewrite-design.md  # 整体架构与跨模块不变量
├── modules/                # 每个 workspace 模块的详细设计
└── requirements/           # 单项需求设计
```

- `docs/` 只提交上列三类正式设计。
- 代码审查/检视报告、交接记录、brainstorm、调研、brief、实施草稿和提示词一律放 `.tmp/`，
  不得加入 Git 或随代码提交。
- 正式文档必须区分“当前实现”“已批准计划”“待决”；不得把目标态写成既成事实。

## 3. 项目范围与架构红线

- 产品名为 `ark`；服务端二进制为 `arkd`。范围包括 ark-agent、Code 模式、UI 自动化插件边界与
  能力总线；Write、Connect、工作流及旧 Electron 运行时不在本仓库恢复。
- `ark-agent` 只能依赖 `ark-ports` 的边界，模型、工具和存储实现由 `arkd` 注入；全进程只有它
  一个模型决策循环。
- `ark-contract` 只放跨进程 wire 契约；端口 trait 放 `ark-ports`；跨 crate 不得穿透内部模块。
- `ark-studio` 只能依赖端口，不得依赖具体后端或模型实现；新增平台能力通过模块或插件扩展，
  核心不得出现平台名称分支。
- `apps/arkd` 是唯一组合根，只负责装配，不写业务规则。
- 详细的依赖方向、存储、并发、插件、HTTP/SSE、配置约束按对应模块设计执行。

## 4. 代码与契约

- 单文件不超过 800 行，单函数不超过 80 行；`lib.rs` 仅精确 `pub use`，禁止 `pub use *`。
- 全仓维持 `#![deny(clippy::all)]`；注释解释原因和约束，不复述代码。
- 新增内容默认 ASCII；Windows 下需精确字节的文本必须 UTF-8 无 BOM。
- Rust 类型是 wire 数据唯一真源。修改 `ark-contract` 后运行：

  ```powershell
  cargo test -p ark-contract --lib ts_export
  git diff --exit-code web/src/generated
  ```

- 前端只使用生成类型；未由 Rust 建模的请求组合和纯 UI 状态放
  `web/src/shared/contracts/web-only.ts` 并注明“非 Rust 契约”。
- 可执行逻辑必须有唯一宿主，禁止 Rust/TypeScript 双实现；ID 使用 newtype，使用量统一为 `Usage`。

## 5. 验证

- 按改动范围执行最小但足够的验证；只改文档至少运行 `git diff --check`。
- 改契约、存储、并发或组合根时，必须运行 `cargo test --workspace`，并覆盖故障路径回归。
- 常用质量门禁：

  ```powershell
  cargo fmt --all -- --check
  cargo check --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```

- 前端改动优先真实浏览器验证；后端改动优先受影响模块的最小编译、测试或接口验证。
- 未运行的验证必须如实说明。不要用 `cargo test ... | Select-String` 判断退出码。

## 6. Git

- 不创建 commit，除非用户明确要求；不覆盖已有用户改动。
- 提交信息使用简体中文，格式为 `[类型]简短描述`；类型为 `feat`、`fix`、`refactor`、`docs`、
  `test`、`chore` 或 `perf`。
- 一次提交只做一件事，不提交 `target/`、`web/dist/`、`web/node_modules/` 或 `.tmp/` 内容。
