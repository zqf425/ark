# ark 开发规范（AGENTS.md）

本文只保留所有改动都必须遵守的流程和跨模块红线。整体架构以
[`docs/rust-rewrite-design.md`](docs/rust-rewrite-design.md) 为准；模块内部的详细设计以
[`docs/modules/`](docs/modules/README.md) 为准。二者冲突时修正文档，不靠口头约定。

## 1. 开发流程

1. **所有修改必须经独立 worktree 提交再合入 `master`**：代码、文档、配置和规范改动都必须在已忽略的
   `.worktrees/<任务名>/` 下从 `master` 创建的 worktree 中完成、验证、检查 diff 并提交；随后将该提交合入
   `master`，再用 `git worktree remove` 删除 worktree。禁止直接在 `master` 修改或提交；并行任务不得共用
   worktree。
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
- 新增或修改用户可见 UI 时必须接入 `web/src/shared/i18n/` 的消息键与中英文资源；不得新增硬编码的用户文案。纯 UI 偏好（语言、字号等）保留在 `web` 本地状态，只有确需跨设备同步时才设计 wire 契约。

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

- 每次变更均按第 1 节的 worktree → 验证 → 提交 → 合入 `master` 流程执行；不覆盖已有用户改动。
- 提交信息使用简体中文，格式为 `[类型]简短描述`；类型为 `feat`、`fix`、`refactor`、`docs`、
  `test`、`chore` 或 `perf`。
- 一次提交只做一件事，不提交 `target/`、`web/dist/`、`web/node_modules/` 或 `.tmp/` 内容。

## 7. 并行与锁竞争

多个 worktree、多个 CLI 代理、正在运行的 `arkd` 同时存在时，本机实测出现过三类锁等待。
按下面规则规避，不要靠重试硬扛。

### 7.1 构建目录按 worktree 隔离

- 一个 worktree 一个 `CARGO_TARGET_DIR`（例如 `$PWD\.tmp\cargo-target`），**禁止**让并行任务共用
  仓库根的 `target/`。共用时 cargo 会打印 `Blocking waiting for file lock on build directory`，
  任务互相排队，先完成的也会被后到的锁拖慢。
- 同一个 target 目录不要同时跑 `cargo check`、`cargo test`、`cargo clippy`：它们争同一把锁。
- 隔离的代价是冷编译，因此只编译受影响包，不要每次 `--workspace`。
- 用 `RUSTC_WRAPPER=sccache` 弥补隔离的冷编译代价：sccache 按编译单元缓存，跨 worktree 复用且不
  共享 target 目录，与上面的隔离规则兼容。注意缓存按 rustc 版本与编译参数命中，工具链升级后
  第一遍仍会全量 miss。临时 target 目录（`.tmp/cargo-target*`）验证完即删，不要长期堆积。

### 7.2 不要对着正在运行的二进制构建

- Windows 上运行中的 `arkd.exe` 或测试二进制会锁住文件，重新构建报
  `failed to remove file .../target/debug/arkd.exe`。
- 验证实例必须用独立 target 与独立端口（如 18792/18794），不要复用桌面端正在跑的
  `target/debug/arkd.exe`；需要重建时先停掉那个实例。

### 7.3 `git` 写操作串行化

- 同一仓库的 `git worktree add/remove`、`git commit`、`git merge` 不得并发，否则出现
  `Unable to create '.../.git/worktrees/<name>/index.lock': Permission denied`，
  或 `worktree remove` 返回 255 而目录仍在。
- 派给沙箱内 CLI 代理（codex/claude）的任务**不得包含 `git commit`**：它们一般只能写 worktree，
  写不了外层 `.git`。提交与合入由发起方统一执行。
- 手工删掉 worktree 目录后必须 `git worktree prune`；否则 `worktree add` 会因“目录已存在”跳过，
  留下只有 `.tmp/` 的空壳，代理读不到源码后直接放弃。
- 复用 worktree 前先校验 `Cargo.toml` 与 `.git` 都存在，否则删掉重建。

### 7.4 门禁命令按耗时分级

- `cargo test --workspace` 会带上 `apps/ark-desktop` 的 Tauri 依赖，单次可超过 10 分钟。
  自动化会话默认用 `cargo test --workspace --exclude ark-desktop`，需要覆盖桌面宿主时单独跑。
- 超时预算按“最长可能耗时”给：批量编译/测试任务留足 2400s 以上，
  否则仍在编译的任务会被误判为卡死（实测出现 `timeout 2400s` 杀掉编译中的会话）。

### 7.5 运行时并发点显式串行

- `POST /v1/threads/{id}/turns` 在启动回合时持有 `workspace_gate`，同一时刻只允许一个回合进入。
  不要在锁内做模型调用或工具执行等长耗时动作，否则会把整个运行时排住。
- 会话写入由 store 的单一 writer actor 串行化，不要在网络层再加一把跨 `await` 的锁。

### 7.6 测试脚本自身的假等待

- SSE 订阅先读 `GET /v1/threads/{id}` 的 `last_seq`，再用 `since_seq=last_seq` 订阅。
  用 `since_seq=0` 会重放历史事件，脚本容易把上一回合的终止事件当成本回合结束。
- 审批状态读 `runtime_snapshot.pending_approvals`，不要假设 `approval_requested` 一发就能立刻
  resolve：实测存在注册窗口期，出现过 404 与 204 两种结果。
- 轮询等待要有上限，并区分“还在编译”与“真卡死”：同时看日志增长与进程 CPU，不要只看日志大小。
