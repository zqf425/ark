# ark Rust 重写项目交接文档

> 更新时间：2026-10-27
> 仓库：`D:\code\github\ark`
> 当前分支：`master`
> 当前基线：`739ba64a57aa08f22efab55b4d308c234663530d`（`[fix]ark Rust 运行时修复 4 个 P0 与 10 个 P1 缺陷`）
> 文档用途：让下一位 Agent 能从当前代码继续实施，不再把本项目误判为“只有设计、尚未开工”。

## 1. 当前结论

`ark` 已经是一个可编译、具备本地 HTTP/SSE 服务、Agent Reactor、模型调用、工具执行、会话存储、子代理、设置接口和 React Code 界面的独立 Rust workspace，不再只是设计稿。

当前进度可概括为：

- Rust workspace、`arkd` 组合根和 Code Web 骨架已建立。
- M0、M2、M5 主体已落地。
- M1、M3、M4、M6、M6.5、M6.6、M6.8、M8 已部分落地。
- M1.5、M6.7、M7、M7.6、M9 基本未落地。
- 2026-10-27 审查发现的 4 个 P0、10 个 P1 已在提交 `739ba64` 修复并提交。
- 当前仍不应宣称“可上线”：完整契约等价、真实审批/用户输入闭环、迁移工具、能力总线装配、Studio、后端插件、Tauri 外壳、灰度迁移和全量质量门禁仍未完成。

接手原则：**先补齐已有主链路的闭环与门禁，再横向增加插件或产品能力。**

## 2. 接手时先执行

```powershell
Set-Location D:\code\github\ark
git status --short --branch
git log --oneline --decorate -5
cargo --version
rustc --version
```

本交接生成前的仓库状态为干净的 `master`：

```text
## master
739ba64 [fix]ark Rust 运行时修复 4 个 P0 与 10 个 P1 缺陷
7a4f26d [feat]ark Rust 运行时首次提交
```

如果接手时状态不同，以新的 `git status` 和实际 diff 为准；不要覆盖其他 Agent 的改动。

本机此前使用的 Rust 环境：

```text
RUSTUP_HOME=D:\toolchains\rustup
CARGO_HOME=D:\toolchains\cargo
rustc/cargo 1.99.1
host x86_64-pc-windows-gnu
```

`D:\toolchains\cargo\config.toml` 曾配置 rsproxy.cn sparse registry。环境变量属于本机配置，不应写入仓库。

## 3. 规范与事实来源优先级

出现冲突时按以下顺序判断：

1. 当前代码、测试和真实命令结果。
2. `docs/rust-rewrite-design.md` v0.6：架构和产品决策的唯一真源。
3. `AGENTS.md`：从设计文档提炼的执行规范。
4. 最新代码审查及修复记录。
5. 早期 brainstorm、brief 和历史 handoff。

重要文档：

| 文档 | 作用 | 当前使用方式 |
|---|---|---|
| `docs/rust-rewrite-design.md` | v0.6 总设计、M0–M9、路由和事件全集 | 唯一设计真源；改架构前必读对应章节 |
| `AGENTS.md` | 分层、依赖、契约、存储、并发、验证和 Git 规范 | 日常开发必读；冲突时回到设计文档 |
| `docs/ark-code-review-2026-10-27.md` | 4 个 P0、10 个 P1 及 P2 清单 | P0/P1 已标注修复；P2 仍是有效候选待办 |
| `docs/ark-code-review-brief.md` | 上述审查的上下文简报 | 只作背景，不替代报告结论 |
| `docs/ark-rs-subagent-codex-review.md` | 较早的子代理专项审查 | 描述的是修复前状态；不能直接当当前缺陷清单 |
| `docs/ark-rs-subagent-codex-review-brief.md` | 子代理审查简报 | 历史背景 |
| `docs/rust-rewrite-brainstorm.md` | 能力总线、插件与隔离思路 | 设计参考 |
| `docs/rust-rewrite-brainstorm-brief.md` | brainstorm 简报 | 历史背景 |
| 本文 | 当前实现与下一步交接 | 每次重要阶段完成后更新 |

原来的本文仍写着“代码尚未开始”，该信息已经失效；本版本已按 `master@739ba64` 重写。

## 4. 项目范围与硬边界

### 4.1 本仓库范围

- Rust Agent 运行时与本地服务端。
- Code 模式 React 渲染层。
- UI 自动化核心的目标架构与后续实现。
- HTTP、MCP、UI、插件、配置、凭据等能力总线。
- 可插拔模型 Provider、Skills、子代理与后端端口。

### 4.2 当前明确不迁移的产品范围

- Write。
- Connect / IM 的旧产品实现（新架构只预留插件方向）。
- 旧工作流产品。
- 旧 Electron 运行时、provider 切换器、诊断面板、CodeWhale / Reasonix 适配器、进程管理器和 RPC 桥。

不要从 `smart-ark` 仓库复制整套旧架构回来。旧仓库只能作为行为、协议、UI 和迁移数据的参考。

### 4.3 不得破坏的稳定边界

- GUI 与 `arkd` 只通过本地 HTTP + SSE 通信。
- 对外路由、字段名、查询参数和事件语义属于兼容契约。
- `since_seq` 等既有查询参数不能随意改成 camelCase。
- 稳定 system prompt 前缀必须保持 byte-stable，动态 workspace、时间戳、文件片段和一次性上下文不得进入稳定前缀。
- 子代理深度 v1 上限为 1，child 工具表不得包含 `delegate_task`。
- 同一数据目录同时只能有一个写进程。

## 5. Workspace 结构

根 `Cargo.toml` 当前包含 19 个 library crate 和一个二进制：

```text
ark/
├── Cargo.toml
├── Cargo.lock
├── AGENTS.md
├── apps/
│   └── arkd/                 # 唯一组合根和服务端二进制
├── crates/
│   ├── ark-common
│   ├── ark-contract
│   ├── ark-ports
│   ├── ark-config
│   ├── ark-store
│   ├── ark-agent
│   ├── ark-llm
│   ├── ark-tool
│   ├── ark-http
│   ├── ark-ui
│   ├── ark-mcp
│   ├── ark-plugin
│   ├── ark-skills
│   ├── ark-provider
│   ├── ark-workspace
│   ├── ark-delegation
│   ├── ark-studio
│   ├── ark-obs
│   └── ark-server
├── web/                      # React + TypeScript + Zustand + Vite
└── docs/
```

### 5.1 依赖方向

设计硬约束：

```text
ark-common      -> 无内部依赖
ark-contract    -> ark-common
ark-ports       -> ark-common, ark-contract
ark-agent       -> ark-common, ark-contract, ark-ports
具体实现 crate  -> common/contract/ports/config 等允许依赖
arkd            -> 所有需要装配的具体实现
```

重点：

- `ark-agent` 不能依赖 `ark-llm`、`ark-tool`、`ark-store` 的具体实现。
- `ark-contract` 只放跨进程 wire 契约；trait 放 `ark-ports`。
- `ark-studio` 应只经 `BackendProvider` 等端口访问后端。
- 只有 `apps/arkd` 可以知道全部具体实现。
- crate 内部模块默认 `pub(crate)`；`lib.rs` 只做精确导出，不用 `pub use *`。

## 6. 模块实施状态

下表区分三种状态：

- **已接入**：代码已由 `arkd` 或 Web 实际生产路径使用。
- **库级实现**：crate 内有实质实现和测试，但尚未完整装配到产品主链路。
- **占位/未实现**：只有接口、空 crate 或返回 `NotFound`/未实现响应。

| 模块 | 当前状态 | 已实现内容 | 主要缺口 |
|---|---|---|---|
| `ark-common` | 已接入 | 错误、强类型 ID、编码、路径/文本工具、Usage 基础类型 | 部分入口缺统一 clippy deny；继续控制公共函数膨胀 |
| `ark-contract` | 已接入 | HTTP/SSE/配置/事件/Item DTO；ts-rs 生成 TS | schemars JSON Schema 未接；部分兼容字段仍需逐项核对 |
| `ark-ports` | 已接入 | Model、Tool、Store、Event、Subagent、Backend、平台服务等 trait | 端口存在不代表实现已装配 |
| `ark-config` | 已接入 | 配置加载/持久化、快照、`ReloadClass`、热更新分类 | 需继续做端到端最小重启验证 |
| `ark-store` | 已接入 | 会话目录、`events.jsonl` 权威日志、writer actor、投影、child runs、reconcile、数据目录锁 | 当前索引是内存 + `data/index.json`，未实现设计的 SQLite/WAL/FTS5；迁移 CLI 缺失 |
| `ark-agent` | 已接入 | 唯一 Reactor、TurnMachine、工具循环、预算、取消、steer、并行、journal | 审批/用户输入当前未接真实 gate；部分路线能力未闭环 |
| `ark-llm` | 已接入 | HTTP/mock client、provider family、流式解析、retry、history repair、usage | `PricingTable`/真实 cost 仍缺；需更多协议差分测试 |
| `ark-tool` | 已接入 | bash/edit/find/grep/ls/read/write、delegate_task、sandbox、真实路径边界、截断 | `git_inspect`、`run_script`、`lsp` 等设计工具未实现；OS 级 shell 隔离不足 |
| `ark-http` | 库级实现 | allowlist、凭据引用、header 防护、redirect/retry/rate limit、统一出口 | 未见 `arkd` 完整实例化并供插件使用 |
| `ark-ui` | 库级实现 | 页面/面板 registry、Code/Changes/Plan/File/Terminal 元数据 | 未见 `arkd` 真实装配 `UiBus`；Web 面板仍多为空态 |
| `ark-mcp` | 部分接入 | 外部 server registry、stdio/SSE/streamable HTTP、catalog、调用深度/环路/死锁防护 | `arkd` 主要接设置 CRUD；`McpBus` 与 MCP 服务端能力面未完整装配 |
| `ark-plugin` | 库级实现 | manifest、版本/拓扑、宿主签发 identity、隔离 config/KV/files/credential/log/metrics handle | 没有真实插件加载流程、参考插件和 `xtask new-plugin` |
| `ark-skills` | 已接入 | SKILL.md discovery、front matter、catalog、触发/预算；父子回合注入 | HTTP 公开能力及完整前端体验仍需按路由核对 |
| `ark-provider` | 已接入 | Provider CRUD、校验、secret masking、probe、按回合 resolve | 继续补真实 provider 兼容测试和错误诊断 |
| `ark-workspace` | 部分接入 | workspace 校验、路径解析、超时 git status | 设计路由兼容面需核对；文件/右侧面板尚未闭环 |
| `ark-delegation` | 已接入 | 并发门、队列、持久化、后台 supervisor、hydrate、abort/steer/resume、child activity | 继续补异常/重启/删除压力测试；历史审查文档尚未更新状态 |
| `ark-studio` | 占位 | crate 可编译，保留模块边界 | UI 自动化 IR、RunBundle、sidecar、gate、执行器均未实现 |
| `ark-obs` | 部分接入 | tracing 与 provider usage 累计 | AuditWriter、metrics/export、完整安全审计主干未完成 |
| `ark-server` | 已接入 | Axum 路由、鉴权、SSE、graceful shutdown、turn/child 管理 | 多个兼容路由仍明确返回未实现；直接依赖 `ark-obs` 是设计 P2 偏差 |
| `apps/arkd` | 已接入 | CLI、store/reconcile、Provider/Skills/MCP registry、父子 Reactor、delegation hydrate、HTTP state 装配 | 目前 gate 使用 `AllowAllGates`；能力总线和插件宿主未完整装配 |
| `web` | 已接入 | React Code 工作台、线程列表、SSE 会话流、Composer、中断、设置页、门禁卡 UI、三栏布局 | 审批 API 后端未闭环；Changes/Plan/File/Terminal 多为空态；没有 Tauri 外壳 |

## 7. 已完成的端到端能力

### 7.1 `arkd` 本地服务

入口：

- `apps/arkd/src/main.rs`
- `apps/arkd/src/cli.rs`
- `apps/arkd/src/wiring.rs`

CLI 当前仅支持：

```text
arkd serve [--port <p>] [--data-dir <dir>] [--workspace <dir>] [--auth-token <t>]
arkd help
arkd version
```

默认值：

- 端口：`18790`
- 数据目录：`~/.ark/data`
- workspace：当前目录
- 无 token 时：仅本地模式，由服务端安全边界决定访问

尚无 `export`、`import`、`import-jsonl` 等迁移子命令。

### 7.2 Agent 与模型回合

已接通：

- 每个父回合由同一套 `ark-agent::Reactor` 驱动。
- `ark-llm` 提供模型请求、流式解析、usage 与消息历史修复。
- `arkd` 每个新回合读取最新 ConfigSnapshot，model/tuning 热更新对后续回合生效。
- 运行中的回合保持创建时快照，不被中途配置变更破坏。
- 支持 interrupt、steer、工具调用、用量和事件落盘。
- stable system prompt 前缀与动态上下文边界已设计并实现基础机制。

### 7.3 会话存储与 SSE

已接通：

- 会话目录和 `events.jsonl` 是权威记录。
- `index.json` 只作可重建派生缓存。
- 每个 thread 的 writer actor 串行分配 seq、append 和 fanout。
- SSE 使用 `since_seq` 增量重放；快照和 live 注册在序列化边界内完成。
- live channel 有界，慢客户端与断开客户端不会无限占用资源。
- `sse_idle_timeout=0` 时，客户端断开也能取消 pump。
- `FileSessionStore::open` 持有 `data/.lock` 跨进程独占锁，阻止两个 `arkd` 同写一个数据目录。

注意：当前存储实现没有达到设计中的 SQLite/WAL/FTS5 索引方案，不能把内存索引 + JSON index 描述为最终实现。

### 7.4 工具与沙箱

已接通的工具：

- `read`
- `write`
- `edit`
- `find`
- `grep`
- `ls`
- `bash`
- `delegate_task`

已修复的安全边界：

- 文件路径不再只做词法前缀比较，而是用真实路径校验已有目标或最近存在父目录。
- workspace 内 symlink / Windows junction 指向外部时会拒绝访问。
- 子代理 workspace 强制继承父 `ToolContext.workspace`，模型传入的 workspace 不可扩权。
- `SandboxDecision` 单次计算并传入执行阶段，审批后不再二次计算导致行为不一致。

剩余风险：

- `bash` 仍主要依赖 current directory 和策略判定，不是完整 OS sandbox。
- Unattended 模式若开放给产品，需 env allowlist、secret stripping 和 OS 级隔离。
- 设计工具 `git_inspect`、`run_script`、`lsp` 尚未实现。

### 7.5 子代理

已接通：

- `delegate_task` 前台和 `background` 后台模式。
- `resumeChildId` 恢复入口。
- runtime-owned supervisor，父回合 future 被丢弃时 child 不会无主。
- queued/running/terminal 状态持久化。
- 启动 `hydrate()` 从权威日志恢复 registry。
- abort、steer、resume、destroy child。
- 排队前注册控制句柄，queued child 可中止。
- steer 在安全边界注入 Reactor 历史。
- child cancel/steer bridge 在回合结束后清理。
- 同一父目录的 append/delete/index rebuild 使用父级锁串行。
- registry 缺失时 child transcript/events fail-closed。
- child events 使用兼容的 `since_seq`。
- child 工具表不含 `delegate_task`，深度保持 1。

权威日志：

```text
threads/<parent>/children/runs.jsonl
threads/<parent>/children/<child>/...
```

`runs.jsonl` 使用每个 child 最新一行判定状态；历史 queued/running 行不会被回写修改。

### 7.6 Provider、Skills、配置和设置

已接通：

- Provider CRUD、校验、脱敏和连通性测试。
- 每个新回合按最新配置 resolve provider/model。
- Skills 扫描、诊断、catalog、触发和父子回合注入。
- MCP server 设置管理与连接测试。
- runtime config 查询和 patch。
- Web 设置页面包括 Provider、MCP、Skills、Subagent 等区域。

### 7.7 React Code Web

入口：

- `web/src/main.tsx`
- `web/src/App.tsx`
- `web/src/features/code-session/`
- `web/src/features/settings/`
- `web/src/shared/api/client.ts`

已实现：

- thread 列表和新建。
- 会话事件 SSE、断线重连和 seq 水位。
- 用户/助手消息流、工具卡片、错误显示。
- Composer、发送、interrupt。
- approval/user-input 卡片的渲染与交互结构。
- runtime/SSE 状态显示。
- 三栏布局与右侧 Tab。
- 设置页面及对应 API hooks。

限制：

- Web 有门禁卡 UI，不代表后端真实 gate 已接通；`arkd` 当前仍使用 `AllowAllGates`，相关 API 仍有未实现入口。
- Changes、Plan、File、Terminal 主要仍是空态或缺少读取接口。
- Web 是浏览器渲染层，不是 Tauri 桌面外壳。

## 8. Rust → TypeScript 契约生成

### 8.1 当前机制

Rust wire 类型是唯一真源：

```text
crates/ark-contract/src/*.rs
  -> ts-rs
  -> web/src/generated/*.ts
```

生成命令：

```powershell
cargo test -p ark-contract --lib ts_export
```

生成结果当前共 52 个文件（包含 `index.ts` 和 `serde_json/JsonValue.ts`）。根类型清单位于：

- `crates/ark-contract/src/ts_export.rs::export_roots()`
- `crates/ark-contract/src/ts_export.rs::ROOT_NAMES`

新增跨进程 DTO 时必须同时登记两处，并重新生成。

### 8.2 Web 契约规则

- `web/src/generated/` 不得手改。
- `web/src/shared/contracts/{ids,item,dto,error,event,settings}.ts` 只做生成类型 re-export。
- Rust 未建模的 UI-only 类型、路由组合请求或投影视图放 `web/src/shared/contracts/web-only.ts`。
- 不得在 Web 重新声明 Rust 已拥有的 DTO 或错误码 union。
- 前端直接使用正式生成类型名，例如：
  - `RuntimeConfigView`
  - `SubagentSettings`
  - `ApplyResult`
  - `ProviderTestResponse`
  - `McpServerTestResponse`
  - `SkillSummary`
  - `SkillDiagnostics`
  - `SkillLoadWarning`
  - `McpTransportKind`

校验：

```powershell
cargo test -p ark-contract --lib ts_export
git diff --exit-code -- web/src/generated
Set-Location web
npm run typecheck
npm run check:rules
npm run build
```

`web/scripts/check-hard-rules.mjs` 会双向检查生成文件头和手写契约重复声明。

### 8.3 尚未完成

- schemars JSON Schema 生成明确 deferred，当前没有消费方。
- `web/README.md` 中仍有“shared/contracts 是生成目录/待生成”等旧描述，应更新为当前 `generated + re-export + web-only` 结构。

## 9. 2026-10-27 P0/P1 修复状态

提交 `739ba64` 已修复并提交以下内容。

### 9.1 P0（全部关闭）

1. 数据目录跨进程独占锁。
2. 子代理 workspace 越权。
3. symlink / junction 路径逃逸。
4. 父回合 panic、abort、Ctrl-C 后不收敛终态。

### 9.2 P1（全部关闭）

1. queued child 未计入活动任务。
2. `children/runs.jsonl` 尾部半行吞后续记录。
3. child 终态写入/派生索引失败导致内存卡 running。
4. 活动父 thread 删除后生成幽灵目录。
5. SandboxDecision 二次计算。
6. steer 未进入 Reactor 历史及后台/resume 委派缺失。
7. model/tuning 热更新对后续父回合不生效。
8. `Usage::merge` 在缓存字段不完整时虚构命中率。
9. Rust→TS 契约生成和 Web 手写 DTO 漂移。
10. SSE idle timeout 为 0 时遗留 pump/subscriber。

详见 `docs/ark-code-review-2026-10-27.md`。该报告已在每项中标记“已修复”。

### 9.3 修复提交记录的验证

提交说明记录以下命令通过：

- `cargo fmt --all -- --check`
- `cargo check --workspace`（有 2 条既有 unused import warning）
- 定向测试：
  - `ark-contract` 7
  - `ark-store` 105
  - `ark-tool` 96
  - `ark-delegation` 29
  - `ark-server` 39
  - `ark-agent` 118
  - `ark-llm` 121
  - 全部 0 失败
- Web：`npm run typecheck`、`npm run check:rules`、`npm run build` 通过。

未完成：

- `cargo clippy --workspace --all-targets -- -D warnings` 未执行；本机未安装 clippy，安装受网络限制中断。
- 提交说明没有声称 `cargo test --workspace` 全量通过，仅列出了受影响 crate 的定向测试。

下一位 Agent 不要把“提交中记录的历史验证”写成自己本轮重新执行的结果。

## 10. 设计里程碑状态

状态是基于当前源码的工程判断，不替代设计文档的正式退出标准。

| 里程碑 | 当前判断 | 说明 |
|---|---|---|
| M0 契约冻结 | 主体完成，仍有尾项 | Rust DTO + ts-rs 已接；JSON Schema deferred；仍需持续做旧协议差分 |
| M1 Store/Server 骨架 | 部分完成 | 会话、事件、SSE、reconcile 已有；SQLite/FTS、历史数据无损迁移验收未完成 |
| M1.5 export/import | 未实现 | CLI 没有 export/import/import-jsonl |
| M2 LLM + 最小 Agent | 主体完成 | Reactor + LLM 已由 arkd 实际装配；旧 TS 差分证据仍不足 |
| M3 工具系统 | 部分完成 | 7 个内置文件/shell 工具已接；git_inspect/run_script/lsp 缺 |
| M4 门禁/会话控制 | 部分完成 | interrupt/steer/compact 有基础；fork/approval/user-input/resume-thread/rewind/review 等未闭环 |
| M5 子代理 | 主体完成 | delegation、事件、管理路由、后台、恢复、hydrate 已接并做过专项修复 |
| M6 扩展能力 | 部分完成 | MCP registry、skills、usage、provider/config 热更新有；attachment/memory 缺，MCP bus 未完全装配 |
| M6.5 能力总线/插件 | 库级部分完成 | http/mcp/ui/plugin crate 有实现；组合根、参考插件、脚手架和服务端能力目录缺 |
| M6.6 平台服务/隔离 | 库级部分完成 | scoped handle 与隔离规则存在；真实多插件端到端未验证 |
| M6.7 IM 插件 | 基本未实现 | 无微信/飞书/钉钉/Telegram 插件全链路 |
| M6.8 配置分级 | 部分完成 | ReloadClass、snapshot、patch、draining 基础已有；完整重启/SSE 验收待做 |
| M7 Studio | 未实现 | `ark-studio` 目前为空壳；Node sidecar 和单 Agent UI 自动化链路未迁移 |
| M7.6 BackendProvider 插件 | 基本未实现 | 端口存在；local/galaxy 插件与切换链路未落地 |
| M8 Code + Tauri | 部分完成 | React Code Web 已有；右侧能力不完整；无 Tauri 外壳和完整交互走查 |
| M9 灰度 | 未开始 | 无双运行时灰度、切流、回退和一个月指标验证 |

## 11. 明确未实现的 HTTP/产品能力

`crates/ark-server/src/api/handlers/pending.rs` 明确保留了尚未落地的兼容路由。接手时应以 `api/routes.rs` 的实际绑定为最终判断，重点核对：

- thread fork。
- goal / todos。
- approvals。
- user inputs。
- memory。
- attachments。
- resume-thread。
- rewind。
- review。
- LLM round diagnostics。
- 部分 workspace/skills/child/config 兼容入口可能已经由其他 handler 替代；不要只看 `pending.rs` 的函数是否存在，要看路由当前绑定到哪个 handler。

当前 `arkd` 父/子 Reactor 使用 `AllowAllGates`。因此“Web 能显示审批卡片”和“真实审批会阻塞并等待用户决定”是两回事；后者仍需实现。

## 12. 下一步开发优先级

### P0：建立可重复的完整质量基线

1. 安装与当前 toolchain 匹配的 `rustfmt`、`clippy` 组件。
2. 在干净环境真实执行：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

3. 修复所有 warning，不再保留“既有 warning”作为长期基线。
4. 加入 CI：Rust 全套门禁、契约生成无 diff、Web typecheck/rules/build。
5. 把 Windows GNU toolchain 和代理镜像只写入开发文档，不硬编码本机路径。

原因：当前最新提交通过定向测试，但没有完整 clippy 和全 workspace 测试证据。

### P0：补真实审批与用户输入闭环

1. 用真实 ApprovalGate/UserInputGate 替换 `AllowAllGates`。
2. registry 保存 pending gate 和生命周期句柄。
3. 实现并测试 `/v1/approvals/{id}`、`/v1/user-inputs/{id}`。
4. 覆盖 approve/deny/cancel/timeout/进程关闭/恢复。
5. 与 Web 卡片端到端联调。

这是工具安全边界和 Code 基本可用性的核心，不应继续靠自动允许模拟。

### P1：完成会话兼容能力与迁移闭环

按设计和旧 GUI 依赖逐项完成：

- fork。
- resume-thread。
- rewind。
- review。
- goal/todos。
- export/import/import-jsonl。
- 旧 Node 会话目录到 Rust 目录的无损迁移、父子归属校验和恢复。

测试必须包含损坏输入、重复记录、相对路径逃逸、迁移中断和回滚，不只测 happy path。

### P1：收敛契约与 Web 主链路

- 更新 `web/README.md` 的旧生成说明。
- 补齐所有仍由 `web-only.ts` 表达但应属于 wire contract 的类型。
- 决定并实施 schemars JSON Schema 消费场景；若继续 deferred，在 CI/文档中保持明确。
- 完成 Changes、Plan、File、Terminal 的后端接口和 Web 接线。
- 保持生成文件不可手改和 stable prompt prefix byte-stable。

### P1：把能力库真正装配成能力总线

- 在 `arkd` 组合根装配 `HttpBus`、`McpBus`、`UiBus`、`PluginRegistry`。
- 建立宿主签发 `PluginIdentity` 的唯一入口。
- 实现插件发现、依赖拓扑、启停、故障隔离和能力目录。
- 提供 `plugin-code-ui`、`plugin-backend-local`、`plugin-studio` 三个参考插件。
- 增加 `cargo xtask new-plugin`。
- 验证插件之间配置/KV/凭据/文件隔离和依赖服务 fail-closed。

不要让插件自行 new HTTP client、MCP server 或 UI container。

### P1：实现 Studio M7

`ark-studio` 当前为空壳，应按设计 §7.8 分阶段完成：

1. case IR 与 RunBundle 契约。
2. Node/Playwright sidecar 协议。
3. runner event v1 兼容。
4. gate、取消、超时、制品和审计。
5. 把旧 generator/healer 的有效 prompt 转为 ark tool/skill，不保留第二层 Agent loop。
6. 用现有真实用例做单 Agent 与旧双 Agent 的差分验证。

不建议直接从旧仓复制整个 Studio 主进程实现；先冻结协议和最小可运行 vertical slice。

### P2：修复审查报告剩余结构问题

`docs/ark-code-review-2026-10-27.md` 的 P2 包括：

- `ark-server` 直接依赖 `ark-obs::UsageRecorder`，应改为端口注入。
- `ChildRunAggregates` camelCase 契约检查。
- SSE 控制帧 id 可能倒退。
- `cost_usd` 恒为 `None`，缺 `PricingTable`。
- Unattended bash 的 OS 隔离与环境变量继承风险。
- 超过 800 行的文件与超过 80 行的函数。
- 部分 crate 入口没有 `#![deny(clippy::all)]` 或 workspace lint 兜底。

优先处理安全、契约和依赖方向，再做机械拆文件。

### P2：后端插件、IM、Tauri 与灰度

在前述主链路稳定后再推进：

- `plugin-backend-local` / `plugin-backend-galaxy`。
- IM 插件及凭据/审批/回复闭环。
- Tauri 2 外壳决策与实现。
- Write / Connect / 工作流最终去留决策。
- 许可证核查。
- M9 双运行时切流、回退与指标验证。

## 13. 开发规范摘要

### 13.1 Rust 代码

- 单文件不超过 800 行，单函数不超过 80 行。
- 全仓目标是 `#![deny(clippy::all)]` 和 `-D warnings`。
- 注释说明“为什么”和约束来源，不复述代码。
- 新增内容默认 ASCII；中文仅用于用户文案和文档。
- 错误必须转换为稳定 `ArkErrorCode`，不允许 panic 穿透请求边界。
- 异步生命周期必须收敛到终态；不能丢弃会留下 running 状态的 future。
- 多锁顺序固定为：锁表 → 父级锁 → locator。

### 13.2 契约

- ID 使用 newtype，不传裸 `String`/`Uuid`。
- Rust 是 wire DTO 与错误码唯一真源。
- GUI JSON 字段 camelCase；既有查询参数按兼容契约保留。
- 改 `ark-contract` 后必须重新生成 TS 并检查 diff。
- cache hit rate 只由完整 provider 原生 hit/miss 计算；字段不完整时为 `None`。

### 13.3 存储与并发

- `events.jsonl`、`children/runs.jsonl` 是权威；索引可重建。
- 同一父目录 append/delete/index rebuild 必须共用父级锁。
- 写数据目录前必须持有 `data/.lock`。
- 授权与 child 归属不得依赖 `index.json`。
- 子进程、文件句柄、bridge task、临时目录都要有明确所有者和退出路径。

### 13.4 插件

- 新工具、provider、MCP、UI 页面、Studio step、后端平台优先作为插件或端口实现。
- 安全策略、身份、配额、审计属于内核，不可交给插件自行决定。
- `plugin_id` 由宿主注入，插件不得自报。
- 配置、KV、凭据、文件、缓存、工具名和日志必须按插件隔离。

### 13.5 Web

- API 调用只放在 `features/*/api` 或 `shared/api`。
- 组件不得写裸 `/v1/...` 路径或直接 `fetch`。
- UI 复用 `shared/ui`，不另建 Toast/Modal/主题系统。
- 生成契约只读；UI-only 类型集中在 `web-only.ts`。
- 用户可见行为要有 loading/empty/error/disabled 状态。

### 13.6 Windows 编码

PowerShell 5.1 的 `Set-Content -Encoding UTF8` 会写 BOM。精确文本使用：

```powershell
[System.IO.File]::WriteAllText(
  $path,
  $text,
  [System.Text.UTF8Encoding]::new($false)
)
```

读取 UTF-8 使用 `Get-Content -Encoding UTF8`。不要通过管道过滤 cargo 输出后只看 `$LASTEXITCODE`，这会掩盖真实退出码。

## 14. 运行与验证命令

### 14.1 Rust

```powershell
Set-Location D:\code\github\ark
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

定向示例：

```powershell
cargo test -p ark-contract --lib ts_export
cargo test -p ark-store
cargo test -p ark-tool
cargo test -p ark-delegation
cargo test -p ark-server
cargo test -p ark-agent
cargo test -p ark-llm
```

启动服务：

```powershell
cargo run -p arkd -- serve --port 18790 --data-dir .\.tmp\ark-data --workspace D:\path\to\workspace
```

需要鉴权时追加：

```text
--auth-token <token>
```

不要让两个 `arkd` 使用同一 `--data-dir`；第二个进程应被 `.lock` 拒绝。

### 14.2 Web

```powershell
Set-Location D:\code\github\ark\web
npm install
npm run typecheck
npm run check:rules
npm run build
npm run dev
```

开发服务器默认 `http://localhost:5273`，代理 `/v1` 与 `/health` 到 `http://127.0.0.1:18790`。可用 `VITE_ARK_BASE_URL` 指向其他服务地址。

### 14.3 文档-only

```powershell
git diff --check
```

## 15. 最小手工冒烟清单

在自动测试通过后至少验证：

1. 启动 `arkd`，`GET /health` 和 runtime info 正常。
2. 打开 Web，创建 thread、发送纯文本回合并收到 SSE 增量。
3. 执行 read/grep/find/ls 等只读工具。
4. 对 workspace 外路径和 symlink/junction 逃逸确认被拒绝。
5. 中断父回合后 registry 与事件都进入终态，可继续发新回合。
6. 前台 `delegate_task` 正常返回；后台模式立即返回 childId，最终状态可查询。
7. queued/running child 可 abort，steer 能进入后续模型历史。
8. 重启 `arkd` 后历史 child 可列出、查看 transcript/events，并按规则恢复。
9. 第二个 `arkd` 指向同一 data dir 时启动失败。
10. 修改热配置后新回合使用新 model/tuning，在途回合不变。
11. 断开 SSE 后 pump/subscriber 不滞留，重连从 `since_seq` 继续。
12. 重新生成 TS 后 `web/src/generated` 无漂移，Web 三项检查通过。

审批/用户输入目前不应列为“冒烟通过项”，因为后端真实 gate 尚未闭环。

## 16. Git 与提交规范

- 未经用户明确要求不要创建 commit。
- 提交信息使用简体中文，格式：`[类型]简短描述`。
- 类型：`feat`、`fix`、`refactor`、`docs`、`test`、`chore`、`perf`。
- 一次提交只做一件事。
- 不提交：`target/`、`web/dist/`、`web/node_modules/`、临时日志、本机配置。
- 只 stage 目标文件，不使用 `git add .`。
- 禁止无授权执行 `reset --hard`、`clean -f`、强推或删除分支。

示例：

```text
[feat]接通审批与用户输入门禁
[fix]修复SSE控制帧水位倒退
[docs]更新Rust重写项目交接状态
```

## 17. 推荐下一位 Agent 的第一项任务

优先建议：**接通真实 ApprovalGate/UserInputGate，并完成 Web 端到端门禁。**

理由：

- 它是 Code 模式安全执行的核心能力。
- 当前 UI 和契约已有基础，可形成清晰 vertical slice。
- `AllowAllGates` 会掩盖真实产品行为，不适合作为继续扩展工具和插件的基础。
- 完成后可顺带验证状态机、事件、SSE、恢复和关闭路径是否真正闭环。

实施前应先输出一页短计划，明确：

- 当前路由和状态机入口。
- Approval/UserInput 的权威记录位置。
- pending gate 的内存与持久化恢复策略。
- 超时、取消、shutdown 和进程重启语义。
- Web 所需 DTO 和事件。
- 最小单元/集成/E2E 测试集。

如果用户明确要求优先迁移 Studio，则第一步不应直接复制旧实现，而应先完成 `RunBundle + runner-event/v1 + sidecar` 的最小契约和单用例 vertical slice。

## 18. 交接结论

当前 `ark` 已越过“设计验证”阶段，进入“核心运行时已成形、产品闭环和迁移能力待补齐”的阶段。下一位 Agent 最容易犯的错误有三类：

1. 把 crate 存在误当成能力已接入 `arkd`。
2. 把 Web 卡片存在误当成后端审批/用户输入已实现。
3. 继续横向开发插件和 Studio，而没有先建立全 workspace clippy/test、契约生成和主链路安全门禁。

后续工作应坚持：代码与真实测试优先、设计 v0.6 定方向、Rust 契约单一真源、资源生命周期必须收敛、插件只经宿主能力总线、稳定 prompt 前缀不得漂移。每完成一个里程碑，应同步更新本文和对应审查状态，避免再次出现“设计说未开工、代码已经实现”或“库已实现但产品未接线”的信息差。
