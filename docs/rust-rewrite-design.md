# smart-ark 的 Rust 重写：ark 运行时 + Code 模式界面 + UI 自动化（插件化）

> 状态：设计草案 v0.6（已过 codex 两轮评审 + 平台服务头脑风暴 + harness 差距对标）
> 基线：`main` @ `253139de`
> 范围：把原 `kun/`（TypeScript agent runtime，**本方案中统一更名为 `ark`**）、
> Code 模式渲染层、UI 自动化核心用 Rust 重建
> 目标：不是"翻译一遍 TypeScript"，而是**在重写中消除当前已知的架构缺陷**
> v0.2 主线：会话目录自包含可迁移、MCP 双向、一切皆插件、代码组织对齐 `QualityDataCenter`
> v0.3 主线：**全局只保留一个 agent**（UI 自动化不再内嵌第二个 agent）、
> **UI 自动化本身即插件**、**后端通过插件接入（不再写死智测方舟）**、`kun` → `ark` 更名
> v0.4 主线：**ark 是能力总线** —— 统一提供 HTTP 出站 / MCP 开放与互调 / 界面承载
> 三组基础能力；插件只注册能力、复用基础能力，并通过总线互相调用；
> **Code 模式界面本身也是插件**
> v0.5 主线：**平台服务插件 + 多插件隔离 + 最小重启** —— 日志/HTTP/凭据/配置/KV/文件等
> 基础设施做成官方发布的平台服务（三层模型）；所有共用能力默认按 `plugin_id`
> 命名空间隔离（日志带插件名、配置键 `{插件名}.{变量名}`）；
> 对标主流 harness 预留能力端口；**新增 IM 接入插件**；
> **配置只有端口等底层项需重启，其余全部热更新**（§7.15）
> v0.6 主线：**对标主流 harness 找差距** —— 补齐可撤销副作用、waterfall 拦截、
> 运行时不变量、真实 tokenizer、DAP 等 8 项（§7.16 + `docs/harness-gap-analysis.md`）

---

## 0. 一页结论

0. **原始项目地址**：D:\code\github\smart-ark
1. **拆分边界已经天然存在**：当前 GUI 与 agent loop 之间已经是 `HTTP + SSE`，
   UI 自动化执行器也已经是**独立进程 + JSONL 协议**（`packages/runner`）。
   重写的核心工作是**在既有进程边界内换实现**，不需要先重构架构。
2. **全局只有一个 agent**：名为 `ark`（原 `kun` 更名而来）。
   模型决策循环、工具调度、审批、上下文压缩、子代理，全部由这一个 agent 承担。
   UI 自动化**不再自带第二个 agent 循环**（现状见 §2.7 的 S-7），
   它只是 `ark` 的一个插件，向 `ark` 注册工具与步骤执行器。
3. **Rust 接管"确定性、高并发、长生命周期"的部分**：
   agent 循环、模型流式解析、工具执行、事件日志、SSE、存储、Studio 执行编排。
   Playwright 浏览器驱动**继续以 Node sidecar 保留**，放在 `BrowserDriver` trait 之后，
   未来可换 `chromiumoxide`。理由是 Playwright 的 locator 语义（role/label/testId +
   自动等待）重写成本极高、收益极低。
4. **界面层不建议用 Rust GUI 重画**：推荐 **Tauri 2 外壳 + 重写后的 React 渲染层**。
   纯 Rust GUI（egui/iced）在流式 Markdown、diff、虚拟滚动、IME 上会显著倒退。
5. **契约用单一真源生成**，而不是 Rust 和 TypeScript 各写一份。
   但要注意：**生成类型 ≠ 生成可执行逻辑**，两者必须分开验收（见 §6.1）。
   当前代码已因此产生真实 bug（§2.1 的 C-2），这是必须一次性解决的头号问题。
6. **会话目录自包含、可整体搬移**：子 agent 会话直接嵌在父会话目录内
   （`threads/<parent>/children/<child>/`），一个目录就是一个迁移单元。
   这条反向决定存储选型：**目录是系统记录，索引是可重建的派生数据**（§7.4）。
7. **ark 是能力总线，不是插件宿主**：ark 统一提供三组**基础能力**，
   插件只声明"我要用"，不自己实现通信层或界面容器（§7.5）：

   | 基础能力 | 插件用它做什么 | 插件不得自己做 |
   |---|---|---|
   | HTTP 出站（`ark-http`） | 与自己的后端通信（代理/TLS/重试/限流/凭据注入/审计全由 ark 负责） | 自建 `reqwest::Client`、自管代理与证书 |
   | MCP（`ark-mcp`） | `expose` 开放自己的工具；`call` 调用**其它插件已注册**的 MCP 能力 | 自起 MCP server、自己开端口、直连对方地址 |
   | 界面（`ark-ui`） | `register_page` / `register_panel` 展示自己的页面 | 自起窗口、自管路由与登录态 |

8. **MCP 双向但只有一个出口**：作为客户端消费外部 MCP；
   同时所有插件（含内置）把工具注册进 ark 的**唯一** MCP 出口，
   外部客户端只连 ark 一个端点。内部 agent 与插件互调走进程内路由，
   带 `trace_id`、深度限制与循环检测（§7.5.2、§7.6）。
9. **一切皆插件，Code 模式界面也不例外**：
   工具、模型 provider、MCP 连接、Studio 动作、**页面/面板、后端集成**
   统一进插件注册表；`plugin-code-ui` 与第三方页面地位完全相同；
   外部插件走 MCP 协议，内置插件编译期注册；
   新增功能用 `cargo xtask new-plugin` 模板生成（§7.7）。
10. **"剥离后端"的真正含义是后端可插拔**：现状把智测方舟（Galaxy）写死在
    Studio 链路里（`FACADE_PREFIX = '/api/ui-automation/galaxy'`，见 §2.7 的 S-8）。
    重写后 `ark` 只定义**后端集成契约**（认证、用例、执行、制品、回传），
    任意后端以插件形式接入，且插件只经 `ark-http` 与 `ark-mcp` 与外界通信，
    **换后端不改核心**（§7.12）。
11. **代码组织对齐 `QualityDataCenter`**：按业务域切 crate（对应 QDC 的
    `qdc-common` / `qdc-base` / `qdc-llm` / `qdc-mcp` / `qdc-server`），
    模块内统一分层（`api / service / domain / store / config`），
    模块间只经 `ark-common`、`ark-contract` 与 trait 交互；
    既有资产按 §5.4 清单逐项决定复用/移植/重写。
12. **交付方式必须是"可回退的双运行时并行期"**，用差分测试证明行为等价，再切流；
    并行期任一时刻只有一个运行时拥有写权限（见 §9.0、§9.3）。
13. **基础设施也是插件（平台服务）**：日志、HTTP、凭据、配置、KV、文件这些
    多插件共用的能力，做成**官方维护、可独立版本化**的平台服务插件，
    经 ark 平台发布；任何插件安装即得全套基础设施，不用自己实现（§7.5.4）。
    所有平台服务**默认假设多插件并发使用**：身份由宿主注入，
    日志自动带插件名、配置按 `{插件名}.{变量名}` 命名空间、
    存储/凭据/缓存/MCP 工具名一律按 `plugin_id` 隔离（§7.5.5）。
    IM 接入（微信/飞书/钉钉/Telegram）是首个样板业务插件（§7.13）。

## 1. 范围界定

### 1.1 在范围内

| 模块 | 现状位置 | 说明 |
|---|---|---|
| Agent runtime | `kun/src/loop/`、`kun/src/services/` | 回合状态机、工具循环、上下文压缩、steering、中断。**重写后统一命名为 `ark`** |
| 模型适配 | `kun/src/adapters/model/` | `endpointFormat` 多形态、流式解析、usage/cache 解析、重试与 fallback |
| 工具系统 | `kun/src/adapters/tool/` | 内置工具、MCP、沙箱策略、限流、文件变更队列 |
| HTTP/SSE 边界 | `kun/src/server/` | 约 45 条路由、SSE 重放、审批与用户输入闸门 |
| 持久化 | `kun/src/adapters/file/`、`hybrid/` | 事件日志、线程存储、项目投影 |
| Code 模式界面 | `src/renderer/src/components/{chat,sidebar,terminal,todo,plan}`、`Workbench.tsx` | 会话流、工具卡片、审批、diff、会话管理 |
| UI 自动化核心 | `src/main/studio/`、`packages/runner/`、`packages/contracts/` | 用例 IR、执行编排、runner 协议、制品、门禁。**重写后改为 `ark` 的一个插件** |
| 后端集成 | Studio 链路里的 Galaxy 直连（见 §2.7 S-8） | **抽成后端集成契约 + 插件**，不再写死智测方舟 |
| 进程间桥 | `src/preload/index.ts`、`src/main/runtime*` | 从 Electron IPC 换成 Tauri command/event |
| IM 接入 | `src/main/{claw-im-*,feishu-streamer,telegram-runtime,weixin-bridge-runtime}` | 收编为 `plugin-im` 业务插件（§7.13），消费平台服务，旧实现只作行为参照 |

### 1.2 明确不在范围内

- Write 模式、工作流引擎、GitLab/观测等业务面板
  （Connect 手机的 **IM 部分已收编为插件**纳入范围，见 §7.13；其余 claw 专属逻辑不迁移）
- 云端"银河"平台的用例管理与发布（继续按 HTTP 客户端对接，不重写服务端）
- Playwright 内核本身（作为 sidecar 复用）
- 遥测后端（OTLP 导出保持协议不变）

### 1.3 能力保留清单（验收基线）

重写**不允许缺失**的对外能力，按当前路由实测枚举：

- 线程：list / create / get / update / delete / fork / archive / search / resume-session
- 回合：start / get / steer / interrupt / compact / rewind
- 事件：`GET /v1/threads/{id}/events` 的 `since_seq` 重放 + 实时推送 + 心跳
- 闸门：`approvals/{id}`、`user-inputs/{id}`（含 `/v1/user-input/{id}` 兼容别名）
- 目标与待办：`threads/{id}/goal`、`threads/{id}/todos`
- 诊断：`runtime/info`、`runtime/tools`、`skills`、`attachments`、`memory`、`usage`、`debug/llm-rounds`
- 子代理：`child-runs` 全套 + 子会话事件重放
- 热更新：`runtime/model-providers`、`subagents/config`、`runtime/tuning`、`connector-env`
- 生命周期：`/health`（免鉴权）、`/v1/shutdown`（Windows 下优雅停机的唯一可靠通道）
- UI 自动化：用例草稿、RunBundle 落盘、runner 命令集（见 §7.8）、制品上传/回看、执行门禁
- 会话目录：`threads/<thread_id>/` 自包含（含 `children/<child_id>/`），支持导出/导入整体迁移
- MCP：客户端（stdio / SSE / streamable-http）+ 服务端（把线程、回合、审批、用量、Studio 暴露为 MCP 工具）
- 插件：注册表、manifest 校验、内置插件清单、`cargo xtask new-plugin` 模板
- 后端集成：`BackendProvider` 契约（认证 / 用例 / 执行 / 制品 / 结果回传），
  内置 `plugin-backend-local` 与 `plugin-backend-galaxy`（智测方舟）两例，
  第三方后端不改核心代码即可接入
- 单 agent：任意时刻只有 `ark` 一个模型决策循环；UI 自动化不得再启动第二个 agent
- 能力总线：`ark-http`（出站 HTTP）、`ark-mcp`（MCP 出口 + 插件互调）、`ark-ui`（界面承载）
  三组基础能力；插件只调用不自建
- 平台服务：`ark.log` / `ark.config` / `ark.credentials` / `ark.kv` / `ark.files`
  官方发布、安装即得；插件缺失时依赖方 fail closed
- 多插件隔离：`plugin_id` 宿主注入；日志带插件名、配置 `{plugin_id}.{key}`、
  存储 `data/plugins/<plugin_id>/`、凭据 `{plugin_id}/{name}`、MCP 工具 `<plugin_id>.<tool>`
- 能力自描述：`GET /v1/runtime/capabilities`、MCP `tools/list`、前端导航表**同源**（一份 catalog）
- 配置生效：`ReloadClass`（Hot / HotRelisten / Restart）分级；热项经单一
  `POST /v1/runtime/config` 原子生效；仅端口等底层项需重启（§7.15）
- IM 接入：`plugin-im` 一个插件承载微信 / 飞书 / 钉钉 / Telegram（§7.13）

---

## 2. 现状问题清单（重写的设计输入）

这一节是本次重写的**主要动机**。每条都带证据，新架构必须逐条给出对策（见 §8 对照表）。

### 2.1 契约重复与漂移（最高优先级：已经产生真实 bug）

| 编号 | 问题 | 证据 |
|---|---|---|
| C-1 | `secret-redaction.ts` 跨包**逐字节相同**的双活副本，脱敏正则会单侧漂移 | `kun/src/config/secret-redaction.ts` 与 `src/shared/secret-redaction.ts`，md5 均为 `ff3cd5d1…`，各 36 行 |
| C-2 | `gui-plan.ts` 跨包镜像**已漂移**：kun 侧未 `.trim()`，GUI 侧有 `.trim()`。带尾部空格的路径两侧判定相反 | `kun/src/shared/gui-plan.ts:21` vs `src/shared/gui-plan.ts:43`；kun 侧 124 行，src 侧 191 行 |
| C-3 | `KunErrorCode` 三份拷贝（zod enum / 手写 union / 字面量数组）。kun 新增错误码时前端静默不识别 | `kun/src/contracts/errors.ts:11-29`、`src/shared/runtime-error.ts:16-34`、`:54` |
| C-4 | `TokenUsage` 是 `UsageSnapshot` 的真子集，`thread-stats.ts` 自造结构，kun 加字段时**静默丢失** | `src/shared/thread-stats-types.ts:105` vs `kun/src/contracts/usage.ts:11-39` |
| C-5 | `TurnSpanKind`（11 项）与 `TurnItem.kind`（10 项）不一致；`src` 侧完全没复用 kun contracts | `src/shared/thread-stats-types.ts:38-49` vs `kun/src/contracts/items.ts:151-165` |
| C-6 | `proxy-fetch` 两份实现、两侧都装配 | `kun/src/adapters/model/proxy-fetch.ts` 与 `src/main/proxy-fetch.ts`，均 127 行 |

**根因**：没有"契约唯一真源"，靠人肉同步 + 注释提醒。

### 2.2 逻辑重复

| 编号 | 问题 | 证据 |
|---|---|---|
| B-1 | `canonicalize` / `stableShape` **7 份**，函数体逐字等价 | `compat-model-client.ts:2980`、`immutable-prefix.ts:34`、`tool-catalog-fingerprint.ts:44`、`prefix-volatility.ts:213`、`compaction-marker.ts:73`、`tool-storm-breaker.ts:112`、`app-settings-provider.ts:176` |
| B-2 | token 估算 **3 份、口径不同**，导致"压缩阈值"与"历史裁剪阈值"互相打架 | `context-estimator.ts:92`、`model-request-estimator.ts:36`、`request-history-hygiene.ts:467` |
| B-3 | UTF-8 字节截断 **3 份**，其中 1 份无续字节回退，会在多字节字符边界切出 `U+FFFD` | `adapters/tool/truncate.ts:36`、`request-history-hygiene.ts:427`、`review/git-review-target.ts:269` |
| B-5 | `Workbench.tsx` 重复定义 3 个已导出的纯函数，正确的公共实现反而没人 import | `Workbench.tsx:223-250` vs `chat/code-session-helpers.ts:60-86` |

### 2.3 巨型单体（可维护性）

| 文件 | 体积 | 问题 |
|---|---|---|
| `kun/src/loop/agent-loop.ts` | **203 KB** | 模型调用、工具循环、审批、压缩、usage、cache、子代理、恢复策略全在一个类里，单文件方法超过 25 个 |
| `kun/src/adapters/model/compat-model-client.ts` | **129 KB** | 三种 endpoint 形态 + 流解析 + 重试 + 计费 + 参数修复混在一起 |
| `kun/src/server/runtime-factory.ts` | 49 KB | 手工依赖注入，装配顺序隐式，无法单测某个组件 |
| `kun/src/services/turn-service.ts` | 41 KB | 回合生命周期与事件广播耦合 |
| `src/renderer/src/components/Workbench.tsx` | **128 KB** | Code/Write/Connect/Studio 全部模式塞进一个组件 |

体量本身不是罪，但在 TypeScript 里这些文件的**边界靠注释维持**，没有任何编译期约束阻止继续膨胀。

### 2.4 死代码与无效导出

- 确定性死代码 19 项 ≈ 2200 行（A 类清单）
- 全仓 1030 个零引用导出，其中 431 个是值导出
- 24 个文件只有测试引用、生产路径从未接入（如 `kun/src/services/performance-observer.ts`）

### 2.5 SSE 与事件日志

优点（应保留）：已有 `DeliveryWindow` 的"记录空洞而非记录已投递"设计，内存与缺口数成正比；
`compaction holes` 有显式处理；心跳 15s；实时缓冲 512。

问题：

- 事件类型是 46 个 `kind` 的巨型联合（`kun/src/contracts/events.ts:18-62`），
  与 `TurnItem.kind`、`TurnSpanKind` 存在第三处重复描述。
- 同一线程事件同时落在 `events.jsonl` 与内存 `InMemoryEventBus`，
  高负载下两者一致性靠调用顺序保证，没有显式不变量。
- `highestSeq()` 与 `loadEventsSince()` 分别读盘，重放边界存在竞态窗口。
- 有大会话时界面卡顿，输入、发送、停止等动作点击无反应

### 2.6 配置与热更新

- 热更新走 4 条不同的 HTTP 路由（`model-providers`、`subagents/config`、`runtime/tuning`、`connector-env`），
  每条各自实现解析与校验，没有统一的"配置变更"模型。
- `connector-env` 直接改写 `process.env`，语义是"进程全局"，
  与"每线程/每工具作用域"的诉求冲突，且只能靠 allowlist 兜底。
- **配置没有"生效方式"分级**：改监听端口和改日志级别走的是同一套重启流程，
  用户无法预期哪些改动需要重启；也没有机制阻止"顺手把可热更新的改动也做成重启"。

### 2.7 UI 自动化（真实缺陷，重写必须规避）

| 编号 | 问题 | 证据 |
|---|---|---|
| S-1 | runner 子进程 `spawn` **没有 error 监听**，直接导致 Electron 主进程 uncaught 崩溃 | `studio-runner-manager.ts:747-767` |
| S-2 | 审计队列被 rejected promise 毒化：一次写失败后，此后每次解析都失败 | `audit/controller.ts:36-49` |
| S-3 | `typecheckPassed` 门禁全链无真实类型检查，至少 8 处用"编译/保存/跑通"冒充 | `useGenerationFlow.ts:798-801,1184,1249,1330,1963,2012,2084,2258` |
| S-4 | 运行制品与调试现场目录从不清理，`%TEMP%` 无界增长 | `studio-local-artifact.ts:10`、`artifacts.ts:33`、`live-debug/host.ts:40` |
| S-5 | 变量快照（可能含账号口令）明文落盘 | `studio-run-bundle.ts:107-115` |
| S-6 | 门禁字段可由外部任意置真，与 `canPublish` 判定两套逻辑 | `useGenerationFlow.ts:973-977` vs `:2255-2259` |
| **S-7** | **UI 自动化内嵌了第二个 agent 循环**：`kun` 的 scenario orchestrator 与官方 Playwright Test Agent（`generator`/`healer`）各跑一套决策循环，后者还需要回调 `kun` 的模型（`createDesktopAgentModel` → `createStudioAgentConversation` → `/v1/threads/.../turns`）。两层循环互相嵌套，导致：超时预算叠加、决策结果与工具结果难以归因、失败重试路径不可预测 | `src/renderer/src/components/studio/app-runtime.ts:110-130`；`authoring/playwright-agents/desktop-adapter.ts:19-52`；`src/main/studio/playwright-agents/session-host.ts:253-263`（`playwright run-test-mcp-server`） |
| **S-8** | **后端写死智测方舟（Galaxy）**：API 前缀、认证 token、用例/版本/执行/制品全部按 Galaxy 形状硬编码，换后端必须改核心代码 | `studio-generation-api.ts:130`、`galaxy-facade-api.ts:37`、`smart-ark-workspace-api.ts:5`、`app-runtime.ts:64-70` |

S-7 的结论：**重写后只保留 `ark` 一个 agent**。UI 自动化降级为
"一组工具 + 一个步骤执行器 + 一份用例模型"，由 `ark` 直接驱动；
不再引入 Playwright Test Agent 的第二层决策循环（见 §7.8.1）。

S-8 的结论：**后端集成抽象成端口 + 插件**，`ark` 核心只依赖契约，
不依赖任何具体平台的路径、字段或认证方式（见 §7.12）。

---

## 3. 设计原则（对策的总纲）

1. **契约单一真源**：Rust 定义类型 → 生成 TypeScript 声明与 JSON Schema → CI 校验生成物无 diff。
   但要区分两类共享物：**数据契约**可以生成；**可执行逻辑**（归一化、脱敏、截断）
   必须选定唯一宿主（见 §6.1），不能靠"生成类型"假装解决了行为漂移。
2. **状态显式化**：回合生命周期、门禁、审批、子代理状态全部用枚举 + 状态机表达，
   非法状态在类型系统里不可构造，而不是靠字符串约定。
3. **单一 agent**：全进程只有一个模型决策循环（`ark`）。
   任何功能（含 UI 自动化）只能是**工具 / 步骤执行器 / 插件**，
   不得再起第二个 agent 去"自主决策"。如需并行，用子代理（同一套循环 + 预算约束），
   而不是第二套 agent 框架。
4. **失败必须隔离**：任何 I/O 失败（磁盘、子进程、网络）只能影响当前操作，
   不允许毒化队列、不允许让进程 panic 冒泡。
5. **取消是语言级能力**：`CancellationToken` + `select!`，而不是散落的 `aborted` 布尔。
6. **边界即 crate**：crate 的 `pub` 面就是对外契约；
   不跨 crate 暴露内部类型，从结构上消灭"1030 个零引用导出"这类问题。
7. **依赖倒置**：核心（`ark-agent`）只依赖端口 crate；
   所有实现（模型、工具、存储、MCP、UI 自动化、后端平台）由组合根注入。
8. **资源有主**：每个子进程、临时目录、文件句柄都有明确的 Rust 所有者，
   `Drop` 即回收；不再依赖"记得清理"。
9. **扩展只加插件，不改核心**：新增工具、provider、MCP、UI 自动化步骤、
   后端平台，都通过插件注册表接入；核心代码不出现 `if backend == "galaxy"` 这类分支。
10. **等价性可测**：新旧运行时用同一份脚本化场景跑差分，事件流归一化后逐字节对比。
11. **三层能力模型**：能力端口（内核 trait + 安全策略）→ 平台服务插件（官方实现，
    可独立版本化）→ 业务插件。安全策略、身份、配额、审计**永远归内核强制**，
    服务实现只负责具体能力；插件不能以"我是官方服务"绕过授权。
12. **多租户是默认假设**：任何平台服务都按"多个插件同时使用"设计。
    `plugin_id` 由宿主注入、不可自报；命名空间与配额默认强制，
    不提供 manifest 开关去关闭隔离。

## 4. 目标架构总览

```
┌───────────────────────────────────────────────────────────────┐
│  ark-desktop (Tauri 2 外壳)                                    │
│  ┌────────────────────────┐   ┌─────────────────────────────┐ │
│  │  web/ (React + TS)     │   │  Tauri command/event        │ │
│  │  Code 模式会话工作台    │◄──┤  薄适配层：只做转发         │ │
│  └────────────────────────┘   └─────────────┬───────────────┘ │
│         ▲                                   │ spawn/supervise  │
└─────────┼───────────────────────────────────┼─────────────────┘
          │ HTTP + SSE (loopback, token)      │
          ▼                                   ▼
┌───────────────────────────────────────────────────────────────┐
│  arkd (Rust 服务端二进制 = 组合根)                              │
│                                                                │
│  ark-server     axum 路由 / SSE / 鉴权 / 优雅停机               │
│  ark-agent      ★唯一 agent：回合状态机（纯逻辑，无 I/O）        │
│  ark-llm        provider 适配 / 流解析 / usage / 重试           │
│  ark-tool       工具注册表 / 内置工具 / 沙箱 / 限流              │
│  ark-store      会话目录读写 + 可重建索引                        │
│  ark-http       ★基础能力：出站 HTTP（代理/重试/限流/凭据）      │
│  ark-ui         ★基础能力：界面总线（页面注册/路由/主题/鉴权）    │
│  ark-mcp        ★基础能力：MCP 出口与总线（开放 + 互调）          │
│  ark-plugin     插件注册表（工具/provider/MCP/页面/后端）         │
│  ark-studio     UI 自动化**插件**：用例 IR / RunBundle / 门禁     │
│  ark-ports      端口 trait（含 BackendProvider；实现由插件注入）  │
│  ark-contract   跨进程 wire 契约（唯一真源）                     │
│  ark-ports      端口 trait（核心依赖它，实现不被核心依赖）        │
│  ark-common     错误、ID、稳定序列化、UTF-8 截断等纯函数          │
└────────────────┬──────────────────────────────┬───────────────┘
                 │ JSONL over stdio             │ HTTPS
                 ▼                              ▼
      ┌────────────────────┐        ┌──────────────────────────┐
      │ studio-runner      │        │ 后端平台（插件实现）       │
      │ (Node + Playwright)│        │ 智测方舟 / 自建 / 本地      │
      │ 仅浏览器执行，无决策│        └──────────────────────────┘
      └────────────────────┘
```

**四个关键判断**：

1. **只有一个 agent**：`ark-agent` 是唯一决策循环。UI 自动化、子代理、MCP 都是它的
   工具/执行器/适配层，不引入第二套 agent。
2. **Node 不会完全消失**，但被压缩到"只有一个 Playwright sidecar"，
   且该 sidecar **不含任何决策逻辑**（现状的 Playwright Test Agent 循环被移除）。
3. **ark 是能力总线**：出站 HTTP（`ark-http`）、MCP 开放与互调（`ark-mcp`）、
   界面承载（`ark-ui`）、日志等基础能力由 ark 统一提供。
   插件只写领域逻辑与页面，通信、治理、界面容器一律复用总线（§7.5）。
4. **后端与界面都可插拔**：`ark-studio` 只依赖 `ark-ports` 的 `BackendProvider`；
   Code 模式界面本身也注册在 `ark-ui` 上。换后端、加页面都不需要改核心。

## 5. Workspace 与 crate 划分

```
ark-rs/
├── Cargo.toml                     # [workspace]，统一 edition / lint / profile / 依赖版本
├── crates/
│   ├── ark-common/                # ← qdc-common：错误、ID、稳定序列化、UTF-8 工具
│   ├── ark-contract/              # ← qdc-base(dto)：**只放跨进程 wire 契约** + 生成物
│   ├── ark-config/                # ← qdc-base(config/constants)：配置加载 + 热更新
│   ├── ark-store/                 # ← qdc-base(dao)：会话目录读写 + 索引
│   ├── ark-llm/                   # ← qdc-llm：provider 适配、流解析、usage
│   ├── ark-http/                  # ★基础能力：出站 HTTP（代理/TLS/重试/限流/凭据注入）
│   ├── ark-ui/                    # ★基础能力：界面总线（页面注册/路由/主题/事件桥）
│   ├── ark-mcp/                   # ★基础能力：MCP 出口 + 总线（客户端与 server）
│   ├── ark-tool/                  # ← qdc-mcp(executor)：工具执行器、沙箱、限流
│   ├── ark-agent/                 # ← qdc-base(service)：回合状态机、子代理、上下文经济
│   ├── ark-ports/                 # 端口 trait（含 BackendProvider；纯 trait，无实现）
│   ├── ark-studio/                # ← qdc-ai-asset：UI 自动化**插件**（IR/RunBundle/门禁/编排）
│   ├── ark-server/                # ← qdc-server(controller)：HTTP/SSE 入口 + 鉴权
│   ├── ark-obs/                   # ← qdc-monitor：tracing / OTLP / usage 计量
│   └── ark-plugin/                # 插件注册表、manifest、内置插件目录
├── plugins/                       # 可选：编译进主程序的插件 crate（见 §7.7.2）
│   ├── plugin-code-ui/            # ★Code 模式界面插件（注册页面与面板，依赖 ark-ui）
│   ├── plugin-studio/             # UI 自动化插件（依赖 ark-studio）
│   ├── plugin-backend-local/      # 本地文件后端（用例/执行落在会话目录 runs/）
│   └── plugin-backend-galaxy/     # 智测方舟后端集成（旧行为平移）
├── apps/
│   ├── arkd/                      # 启动类（对应 Spring Boot Application）：组合根
│   └── ark-desktop/               # Tauri 2 外壳
├── web/                           # React + TS 渲染层（Code 模式）
└── xtask/                         # 代码生成、契约校验、差分测试、插件脚手架
```

**命名说明**：原 `kun` 全部更名为 `ark`（crate 前缀、二进制 `arkd`、包名、日志、
配置节名）。**不需要兼容**（这是一个全新的项目，轻装上阵，不需要考虑兼容）。

### 5.1 模块内部统一分层（DDD思想，直接对齐 `QualityDataCenter`）

`QualityDataCenter` 的做法是：**按业务域切模块，模块内分层固定**
（`controller` / `service` / `dao` / `dto` / `entity` / `request` / `config` /
`constants` / `utils` / `executor`），模块之间只经 `qdc-common` 与接口交互，
依赖版本集中在父 `pom.xml`。Rust 侧逐条映射：

| QDC 层 | Rust 对应 | 约束 |
|---|---|---|
| `controller` | `src/api/`（axum handler） | 只做参数解包、鉴权、调 service；**禁止写业务分支** |
| `service` | `src/service/` | 用例编排；依赖 trait，不依赖具体实现 |
| `dao` | `src/store/` | 只做持久化；SQL/IO 不向上泄漏 |
| `entity` | `src/domain/` | 领域实体 + 不变量校验 |
| `dto` / `request` | `ark-contract` | 跨模块/跨进程共享的**唯一**结构定义 |
| `config` / `constants` | `src/config/` | 配置结构体 + 默认值，集中管理 |
| `executor` | `src/exec/` | 可插拔执行器（工具、Studio 步骤） |
| `utils` | `ark-common` | 跨模块纯函数；**单仓仅一份** |

每个 crate 固定为：

```
crates/ark-llm/
├── src/
│   ├── lib.rs            # 只 re-export 公共面（pub use 精确到类型，禁止 pub use *）
│   ├── api/              # 仅在 ark-server 出现
│   ├── service/          # 业务编排
│   ├── domain/           # 领域实体
│   ├── store/            # 持久化
│   ├── config/           # 本模块配置
│   └── exec/             # 执行器
└── tests/                # 集成测试
```

硬约束（CI 强制）：

- `lib.rs` 的 `pub use` 白名单化，杜绝现状“1030 个零引用导出”。
- 跨 crate 只能 `use ark_contract::X`，不允许 `use ark_store::internal::Y`（内部模块 `pub(crate)`）。
- 模块之间禁止循环依赖；`cargo deny` 固化方向。
- 单文件 ≤ 800 行、单函数 ≤ 80 行，`xtask lint-size` 落地。

### 5.2 依赖方向（A → B 表示"A 依赖 B"，编译期强制）

```
ark-common     → (无内部依赖，仅 serde/thiserror/uuid)
ark-contract   → ark-common
ark-ports      → ark-common, ark-contract
ark-config     → ark-common, ark-contract
ark-store      → ark-common, ark-contract, ark-ports, ark-config
ark-llm        → ark-common, ark-contract, ark-ports, ark-config
ark-tool       → ark-common, ark-contract, ark-ports, ark-config
ark-http       → ark-common, ark-contract, ark-ports, ark-config
ark-ui         → ark-common, ark-contract, ark-ports, ark-config
ark-mcp        → ark-common, ark-contract, ark-ports, ark-config, ark-http
ark-studio     → ark-common, ark-contract, ark-ports, ark-config   ★只依赖 BackendProvider 端口
ark-agent      → ark-common, ark-contract, ark-ports        ★只到这里
ark-plugin     → ark-common, ark-contract, ark-ports, ark-http, ark-ui, ark-mcp
ark-server     → ark-common, ark-contract, ark-ports, ark-agent, ark-store, ark-config
ark-obs        → ark-common, ark-contract
arkd (组合根)   → 全部（唯一允许"知道所有实现"的地方）
```

约束：

- `ark-agent` **只依赖 `ark-ports`**，不依赖 `ark-llm` / `ark-tool` / `ark-store` 的实现。
  模型、工具、存储由 `arkd` 注入。这直接解决现状 `agent-loop.ts` 与
  `runtime-factory.ts` 互相纠缠的问题。
- `ark-contract` **只放 wire 契约**（HTTP/SSE/MCP/TS 共享的 DTO），
  不放内部 request/query 类型；端口 trait 单独放 `ark-ports`，
  避免把异步 trait 与跨进程契约混在一个 crate 里。
- `ark-studio` 只依赖 `ark-ports::BackendProvider`（端口 trait），
  **不依赖任何具体后端实现**；实现由 `plugins/plugin-backend-*` 提供并在 `arkd` 注入。
- 校验手段：`xtask lint-deps` 读取各 `Cargo.toml`，与上面的白名单逐对比较；
  违规直接失败。`cargo deny` 只负责许可证/重复依赖/安全公告，不承担方向校验。

### 5.3 组合根

只有 `apps/arkd/src/wiring.rs` 允许"知道所有实现"。写法：

```rust
// 显式结构体组装，每个字段都有类型，编译期检查完整性
pub struct Runtime {
    pub contract: Arc<ContractConfig>,
    pub store: Arc<dyn SessionStore>,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
    pub loop_: Arc<AgentLoop>,
    pub bus: Arc<EventBus>,
}

pub fn build(cfg: &ArkConfig) -> Result<Runtime> { /* 只做组装，不含业务分支 */ }
```

对比现状：`runtime-factory.ts` 49KB、按顺序 `const` 拼接、字段间隐式依赖。
Rust 版本里**每个组件可单独构造与单测**，组合根只负责连线。

### 5.4 既有资产复用清单（逐项决策，不重写已验证的东西）

重写的成本必须花在"值得重写"的地方。下表逐项给出决策：

| 资产 | 现有位置 | 决策 | Rust 侧边界 | 兼容验证 |
|---|---|---|---|---|
| 事件契约（46 种 kind、字段形状） | `kun/src/contracts/events.ts` | **平移契约** | `ark-contract::event` 逐一对应，字段名不变 | 差分：同场景事件流逐字段比对 |
| HTTP 路由形状与错误码 | `kun/src/server/routes/index.ts`、`contracts/errors.ts` | **平移契约** | `ark-server` 路由表按附录 A 一一对应 | 路由清单测试：旧新两侧路径/方法/状态码一致 |
| runner JSONL 协议 | `packages/runner/src/protocol.ts`、`contracts/src/runner-event.ts` | **原样复用** | Rust 只做编解码与版本校验，不改字段 | 现有 runner 不改一行即可被 `arkd` 驱动 |
| Playwright 执行能力 | `packages/runner`、`packages/playwright-runtime` | **原样复用（sidecar）** | `SidecarDriver` 只做协议 | 同一 RunBundle 在旧/新编排层下产物一致 |
| 模型请求/响应形状与 usage 解析 | `kun/src/adapters/model/compat-model-client.ts` | **契约复用，实现重写** | 用录制 fixture 驱动 Rust 解析器 | 回放真实响应，字段级断言（含 cache hit/miss） |
| 内置工具语义（read/bash/edit/...） | `kun/src/adapters/tool/builtin-*.ts` | **语义复用，实现重写** | 每个工具一份参数 struct + 行为测试 | 同一输入下输出/副作用一致（含截断与编码） |
| MCP 客户端语义（三种传输、trustScope） | `kun/src/adapters/tool/mcp-tool-provider.ts` | **语义复用** | `ark-mcp` 客户端，行为对齐 | 对同一 MCP server 的调用结果一致 |
| Studio IR 与门禁语义 | `src/renderer/src/components/studio/authoring/`、`shared/studio/` | **语义复用，实现上移 Rust** | `ark-studio` 的 IR 与 `Verified<T>` | 同一用例生成同一 RunBundle（归一化后一致） |
| Code 模式 UI 组件与交互 | `src/renderer/src/components/{chat,sidebar,terminal,todo,plan}` | **代码复用（迁移到 `web/`）** | 只改数据源与 IPC 调用 | 交互走查清单 + 快照对比 |
| 已有测试场景与 fixture | `kun/src/**/*.test.ts`、`packages/runner/src/**/*.test.ts` | **提取为跨语言 fixture** | 存为 JSON 场景文件，双运行时共用 | 差分测试台直接消费 |

三条判据：

1. **协议/契约 → 平移**（保证兼容），**实现 → 重写**（消除缺陷）。
2. 任何"要在 Rust 里重新实现一遍行为"的资产，必须先有**可执行的对照样例**（fixture），
   否则不算可复用，只算"凭记忆重写"。
3. 前端组件**尽量搬运不重写**：Code 模式交互已经被用户验证过，
   重写收益低、回归风险高，只改数据获取层（§7.10.2）。

---

## 6. 契约层设计

### 6.1 单一真源 + 代码生成

先把两类"共享物"分开，它们的解法不同：

| 类别 | 例子 | 唯一真源 | 前端消费方式 |
|---|---|---|---|
| **数据契约**（结构/枚举/错误码） | `RuntimeEvent`、`TurnItem`、`ArkErrorCode` | Rust 类型 + `schemars` / `ts-rs` | 生成 TS 声明，零手写 |
| **可执行逻辑**（纯函数行为） | 路径归一化、密钥脱敏、UTF-8 截断、token 估算 | **选定唯一宿主**（见下） | 只能调用宿主，不得本地重实现 |

生成类型**不能**解决 C-1（脱敏逻辑重复）与 C-2（路径归一化漂移）——
那是函数行为，不是类型。文档 v0.2 把这两条标成"已由代码生成解决"是错的，
v0.3 修正为：

**可执行逻辑的三选一，逐函数显式决策：**

| 方案 | 适用 | 代价 |
|---|---|---|
| R1. 只由 Rust 执行，前端经 Tauri command / HTTP 调用 | 脱敏、路径归一化、截断（调用频次低、含敏感数据） | 每次多一次 IPC/RPC |
| R2. 编译为 WASM（`wasm-bindgen`），前端直接调用同一份 Rust 实现 | 高频纯函数（**路径归一化**、字节截断） | 增加 wasm 构建链 |
| R3. 保留独立 TS 包 + 跨语言测试向量（`fixtures/*.json` 双跑） | 前端必须本地同步执行、且不宜引入 wasm | 仍需维护两份，但由测试向量强制等价 |

默认选择：**路径归一化走 R2，脱敏与截断走 R1**。
理由是归一化在前端每帧都可能调用（必须本地），而脱敏/截断只发生在展示与日志边界。

数据契约部分的写法：

```rust
// crates/ark-contract/src/event.rs
#[derive(Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../web/src/generated/")]
pub enum RuntimeEvent {
    ThreadCreated { seq: u64, thread_id: ThreadId, timestamp: DateTime<Utc> },
    TurnStarted   { seq: u64, thread_id: ThreadId, turn_id: TurnId, timestamp: DateTime<Utc> },
    // ...
}
```

- `schemars` → `schema/*.json`（跨语言校验、MCP 工具 schema 复用）
- `ts-rs` → `web/src/generated/contracts.ts`
- CI：`cargo xtask gen && git diff --exit-code`，生成物有 diff 即失败
- 可执行逻辑：`cargo xtask logic-vectors` 跑跨语言测试向量，任一实现偏离即失败

**C-2 的唯一执行路径（与前文 R2 决策保持一致，不再有两种说法）：**

```text
ark-common::path::normalize()          ← 唯一实现（Rust）
        ├─ 原生调用：arkd / ark-agent / ark-studio
        └─ WASM 调用：web/src/wasm/ark-path  ← 同一份 Rust 源码，wasm-bindgen 导出
```

- 前端**不得**再写任何路径归一化的 TS 实现；`web/` 里只能 `import` WASM 绑定。
- CI 校验：`cargo xtask logic-vectors --target path` 用同一组向量分别跑
  原生与 WASM 产物，输出必须逐字相同。
- 脱敏（C-1）与截断走 **R1**（Rust 侧命令调用），因为它们是低频且涉及敏感数据，
  不适合把脱敏规则下发到前端。

这一条解决 C-3 ~ C-6；C-1 / C-2 由上面的 R1/R2 决策解决。

### 6.2 错误码

```rust
#[derive(Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ArkErrorCode {
    ValidationError, NotFound, Unauthorized, Conflict,
    ModelRequestFailed, ToolExecutionFailed, RuntimeUnavailable, /* ... */
}
```

前端不再手抄 union，也不再需要 `KNOWN_KUN_CODES` 这种二次列表。

### 6.3 ID 新类型

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
pub struct ThreadId(pub Uuid);   // TurnId / ItemId / ChildId 同理
```

防止现状中"把 threadId 传成 turnId"这类只能靠命名约定防的错误。

### 6.4 用量与成本

**只保留一个** `Usage` 结构，包含 provider 原生字段与派生汇率：

```rust
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cache_hit_tokens: Option<u64>,   // provider 原生
    pub cache_miss_tokens: Option<u64>,  // provider 原生
    pub cache_hit_rate: Option<f64>,     // hit / (hit + miss)，缺失则不臆造
    pub cost_usd: Option<f64>,
}
```

- 禁止任何"投影结构"（直接解决 C-4）。
- `cache_hit_rate` 缺失时必须为 `None`，**不允许**用 `hit / prompt_tokens` 推算。
- 计费表（`deepseek-pricing` / `minimax-pricing`）做成 `PricingTable` trait，
  以配置注入，不在客户端里硬编码分支。

---

## 7. 关键子系统设计

### 7.1 Agent Loop：显式状态机

**现状**：`agent-loop.ts` 203KB，`runTurn()` 单个方法跨数百行，
模型调用、工具派发、审批、压缩、usage 记账共用一大段可变状态。

**新设计**：把"回合"建模为状态机，每个状态是独立可测的函数。

```rust
pub enum TurnState {
    Preparing,            // 组装请求前缀 + 修复历史
    AwaitingModel,        // 流式读取
    DispatchingTools,     // 并行/串行派发
    AwaitingApproval,     // 阻塞在闸门
    AwaitingUserInput,    // 阻塞在闸门
    Compacting,           // 上下文压缩
    Finalizing,
    Completed, Failed, Aborted,
}
```

结构：

- `loop_.rs`：状态迁移函数 `step(state, input) -> Action`
- `policy/`：预算、`tool-storm-breaker`、`tool-rate-limit`、并行安全判定，各自独立模块
- `context/`：`TokenEstimator`（唯一实现）、`Compactor`、`HistoryHygiene`
- `reactor`：唯一执行 I/O 的地方，把 `Action` 转成具体副作用

好处：

- 状态迁移可以**纯函数单测**，不需要起 HTTP、不需要 mock 模型。
- 巨型文件问题从结构上消失：单个模块天然 < 800 行。
- 便于加"不变量断言"：例如"进入 Completed 前必须有 terminal 事件"。

**中断语义**：

```rust
let token = CancellationToken::new();
tokio::select! {
    _ = token.cancelled() => { finalize_aborted(); }
    r = model_step() => { /* ... */ }
}
```

`interrupt` 路由只做 `token.cancel()` + 落 `turn_aborted` 事件，
状态机在下一个 `select!` 边界必然退出。不再需要散落的 `aborted` 标志检查。

### 7.2 模型适配

`ark-llm` 内部结构：

```
provider/
  openai_chat.rs      # chat/completions + 流式 SSE
  openai_responses.rs # responses 形态
  anthropic.rs        # messages 形态
  common/
    stream.rs         # SSE 帧解析、增量拼接、断流重试
    usage.rs          # 原生字段优先，兼容字段兜底
    retry.rs          # 指数退避（只此一处）
    repair.rs         # 工具参数修复、历史修复
```

- `endpointFormat` 决定**路由到哪个模块**，而不是在一个函数里堆 `if`。
- 代理（proxy）只有一份实现，通过 `reqwest::Client` 构造注入，
  解决 C-6 的双份 `proxy-fetch`。
- 重试策略集中一处，避免各处各自实现指数退避。
- 流式解析用 `bytes_stream()` + 显式状态机，事件逐块产出，
  不把整段缓冲进内存（现状在长回复上有内存峰值）。

### 7.3 工具系统

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn schema(&self) -> &'static serde_json::Value;   // 启动时 canonical 排序一次
    async fn execute(&self, ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput>;
}
```

- **参数不再靠字符串解析**：`ToolArgs` 是 `serde_json::Value` 经 schema 反序列化后的强类型。
- **schema 指纹在启动时计算一次**并缓存，避免现状"每回合重算 tool catalog fingerprint"。
- 沙箱策略是**决策对象**，在派发前一次性算出 `SandboxDecision`，
  而不是让每个工具自己判断。
- 文件变更队列：`file-mutation-queue` 改为 `tokio::sync::Mutex` 按路径分区，
  同一路径串行、不同路径并行。
- 工具集变更（新增/删除）走 `ToolCatalog` 的版本号，
  版本变化时显式触发前缀失效，而不是靠 fingerprint 事后发现漂移。

### 7.4 存储：会话目录即系统记录

**现状**：`events.jsonl` + `messages.jsonl` + 会话 JSON + `hybrid-thread-store` 里的 SQLite，
三者由不同代码路径写入，一致性靠调用顺序。更要紧的是主/子会话分处两个平级目录
（`data/threads/` 与 `data/child-runs/`），搬走一个会话必须同时搬两处，否则断链。

**新设计**：**目录是系统记录（system of record），索引是可重建的派生数据。**

#### 7.4.1 目录布局

```text
data/
└── threads/
    └── thr_01H.../                     # 一个会话 = 一个自包含、可整体搬移的目录
        ├── thread.json                 # 线程元数据、标题、worktree、provider、model
        ├── session.json                # 当前会话投影（最后一条 turn 的终态）
        ├── events.jsonl                # append-only 事件日志（唯一的真源）
        ├── items.jsonl                 # 渲染就绪的 item 流（可由 events 重建）
        ├── usage.jsonl                 # 用量增量（可由 events 中 usage 事件重建）
        ├── attachments/                # 本会话附件
        ├── runs/                       # UI 自动化 RunBundle 与制品
        │   └── run_01H.../
        │       ├── bundle.json
        │       └── artifacts/
        └── children/                   # ← 子 agent 内联在父会话内
            ├── index.json              # 子会话清单（childId → 状态/标签/父 turn），派生缓存
            ├── runs.jsonl              # 【权威】ChildRunRecord 追加日志，含 parent_thread_id
            └── child_01H.../
                ├── thread.json
                ├── events.jsonl
                ├── items.jsonl
                ├── usage.jsonl
                └── runs/
```

**深度上限：v1 固定为 1 层（父 → 子），不支持孙代理。**

理由（经代码核对）：现状 `child-agent-executor.ts` 在子代理的工具集里
**剔除 `delegate_task` 并显式禁止嵌套委派**，所以"多层嵌套"并不是现有执行能力。
目录结构保留 `children/` 递归的可能性，但 v1 在
`DelegationPolicy::max_depth = 1` 处硬性拒绝，并在错误信息里说明原因。
这样目录布局与执行语义一致，不会出现"磁盘上支持、运行时禁止"的误导。

**存储路由（关键实现点）：**

- **`ChildRunRecord` 的权威存储是父会话目录下的 `children/runs.jsonl`**（append-only）。
  `children/index.json` 只是**派生缓存**（可从 `runs.jsonl` 全量重建），
  不承担权威职责——这与 §7.4 的"目录即真源、索引可重建"保持一致。
- 记录的权威字段：`child_id`、`parent_thread_id`、`parent_turn_id`、`label`、
  `status`、`created_at`、`ended_at`、`model`、`provider_id`。
  归属校验（`owns_child_run`）读 `runs.jsonl`，**不读 `index.json`**。
- 子会话的 store 不能再用"固定 `dataDir/child-runs` 换一个根"的方式构造。
  改为按父线程寻址：`ThreadStore::child_store(&parent_id)` →
  `data/threads/<parent>/children/<child>/`。
- 子会话 ID 全局唯一，因此需要一个**内存中的 `child_id → (parent_id, rel_path)` 映射**，
  启动时从 `threads/*/children/runs.jsonl` 扫描构建，同时重建 `index.json`
  （与 §7.4.2 的索引重建同一趟）。
- 内存映射与 `runs.jsonl` 不一致时（例如手工改过目录），
  **以 `runs.jsonl` 为准**重建，并记一条 `index_repaired` 诊断事件。
- REST 路由 `/v1/child-runs/{childId}` 保持不变：先查映射拿到 `parent_id`，
  再校验 `parent_thread_id` 归属，最后定位目录。授权仍以
  `ChildRunRecord.parent_thread_id` 为准，**不从路径推导授权**。
- `destroy_children_of(parent)`：先 `cancel` 所有运行中的子任务 →
  等每个子会话 writer 落终态并关闭句柄 → 整目录改名 `children.deleting-<uuid>` →
  异步删除。改名而不是直接删，是为了让崩溃后的孤儿清理能识别"删除中"状态。
- 删除父线程时若存在活动子任务，HTTP 返回 **409 + 子任务清单**，
  由图谱确认后再带 `force=true` 重试。这是显式契约，不是静默级联。

**可迁移性边界（区分"能搬"与"搬完能跑"）：**

| 内容 | 随目录迁移 | 说明 |
|---|---|---|
| 事件、item、usage、会话元数据 | ✅ | 相对路径，无外部依赖 |
| 附件 | ✅ | 存在 `attachments/` 内 |
| RunBundle 与制品 | ⚠️ 可选 | 大文件可标记为 `external`，导出时询问是否包含 |
| worktree 路径、绝对工作区路径 | ❌ | 导入后在 `thread.json` 标记 `missing_workspace`，会话可读不可续跑 |
| API key、MCP 环境变量 | ❌ | 永不写入会话目录；导入后要求重新配置 |

导出包格式：`ark-session.tar.zst` + `manifest.json`（声明 portable / non-portable 字段、
是否含敏感内容、原机器标识）。导入时按 manifest 决定是否需要用户补齐。

#### 7.4.2 索引与查询

目录结构适合"单会话读写"，不适合"跨会话搜索"。因此引入**可重建索引**：

```rust
pub struct ThreadIndex { path: PathBuf /* data/index.sqlite */ }
```

**索引表只存元数据，不存事件正文：**

```sql
CREATE TABLE thread_meta (
  thread_id TEXT PRIMARY KEY,
  parent_thread_id TEXT,
  title TEXT, updated_at TEXT, status TEXT,
  tokens_total INTEGER,
  path_rel TEXT,              -- 相对 data/ 的路径
  indexed_seq INTEGER         -- ← 关键：已索引到的 seq 水位
);
CREATE TABLE thread_fts (     -- FTS5，仅存标题与"摘要词项"，见下
  thread_id TEXT, content TEXT
);
```

**崩溃窗口的修复（v0.2 遗漏的关键点）：**

- 每次写入 `events.jsonl` 成功后，索引更新是**幂等**的：
  `UPDATE thread_meta SET indexed_seq = ?`。
- 启动时不只看"索引是否存在"，而是**逐线程对账**：
  `events.jsonl` 的最大 `seq` vs `thread_meta.indexed_seq`。
  - 相等 → 跳过
  - 索引落后 → **增量重放** `(indexed_seq, max_seq]` 更新元数据
  - 索引超前 / 目录缺失 → **全量重建该线程**
  - `thread_meta` 缺行 → 扫描目录发现并插入
- 对账在后台执行，GUI 先显示"索引同步中（N/M）"，不阻塞启动。

**SQLite 写入所有权与并发边界（第三轮评审补齐）：**

- 全进程只有一个 `IndexProjector` actor 持有 `index.sqlite` 的写连接；线程 writer、HTTP handler、
  搜索服务和插件均不得直接执行写 SQL。SQLite 开启 WAL，但 WAL 只用于读写并发，
  **不把它误当成多写者方案**。
- 每个线程 writer 先为 `events.jsonl` 分配 `seq` 并 `fsync`，再发送
  `IndexDelta { thread_id, from_seq, to_seq }`。`IndexProjector` 对同一线程的连续 delta 合并，
  按“最多 256 条或 10 ms”批量开一个 SQLite 事务。
- 元数据、FTS 派生文本和 `indexed_seq = to_seq` 必须在**同一 SQLite 事务**提交；
  禁止先推进水位再更新投影。事务失败只回滚索引事务，不回滚已持久化事件。
- `IndexDelta` channel 有界（默认 4096）。队列满时线程 writer **不等待 SQLite**，
  只把该线程写入一个按 `thread_id` 去重的 dirty set；`IndexProjector` 稍后按事件日志水位补投影。
  dirty set 由 `data/index-dirty.json` 原子快照，进程崩溃后仍可恢复；即使该快照也丢失，
  启动时的 `max_seq` / `indexed_seq` 全量对账仍是最终兜底。
- 索引读取使用只读连接池和短事务；索引重建、FTS 查询、GUI 列表不能占用线程事件 writer。
  队列深度、合并率、事务耗时、dirty 线程数均进入指标和健康诊断。

**FTS 的正文来源必须说明：**

- 现状需要"按标题与消息内容搜索会话"。为避免把事件正文复制进 SQLite（双写不一致），
  FTS 只索引**标题 + 首条用户消息 + 最后一条助手消息**这三段派生文本，
  由 `items.jsonl` 派生，并在 `indexed_seq` 变化时重建该线程的 FTS 行。
- 全文命中后，详情由 `path_rel` 读目录。搜索结果标注"仅概要检索"，
  完整搜索通过 `arkd search --full`（流式扫 `items.jsonl`）提供。

**为什么不以 SQLite 为真源**：真源必须能随目录一起被拷走。
把事件放 SQLite 会让"迁移一个会话"退化成"导出子集再导入"，与需求 1 冲突。

#### 7.4.3 并发与一致性

- **单 writer actor**：每个会话目录一个 writer task，
  独占 `events.jsonl` 追加权。**seq 的唯一权威是 writer 对日志的分配**，
  不是 SQLite（v0.2 在 §7.9 写成"数据库保证"，v0.3 已更正）。
- **快照与订阅原子化（Blocker 4 的修复）**：

  ```rust
  // writer actor 内部，同一串行化点完成三件事
  enum WriterCmd {
      SnapshotAndSubscribe {
          since_seq: u64,
          reply: oneshot::Sender<(Vec<RuntimeEvent>, u64 /*watermark N*/, Receiver<RuntimeEvent>)>,
      },
      Append { event: RuntimeEvent, ack: oneshot::Sender<u64> },
  }
  ```

  actor 在处理 `SnapshotAndSubscribe` 时：读取 `(since_seq, N]` →
  注册"只接收 `seq > N`"的订阅 → 返回。**读与订阅之间没有窗口**，
  不会出现 v0.2 里"读完还没订阅、新事件丢失"的问题。
- **慢客户端隔离**：每个订阅一个**有界** channel（容量 512）。
  满了就**断开该连接**（不是阻塞 writer，也不是静默丢事件），
  由客户端用 `since_seq` 重连补齐。持久化路径永远不会被网络反压。
- **压缩空洞**：压缩保留 `seq` 空洞语义；`SnapshotAndSubscribe` 返回的
  watermark 是"已投递的最大 seq"，客户端按 `seq` 单调性跳过空洞，不需要重算区间。
- **崩溃恢复**：启动时扫 `events.jsonl` 尾部，丢弃不完整的最后一行；
  以文件中最大 `seq` 为准。`items.jsonl` / `session.json` 由事件重放修复。
- **文件锁**：`data/.lock` 上独占锁（`flock` / `LockFileEx`），
  保证同一数据目录只有**一个** writer 进程——这条同时是 §9.3 双运行时规则的实现基础。

#### 7.4.4 事件日志治理

- 保留期策略：`usage` 事件默认 365 天、活动流（delta）默认 8MB / 2048 条上限，
  对齐现状阈值，但改写为**单会话内的顺序重写**（actor 内完成），
  不再有"并发 compact 与 append 竞争"的问题。
- 裁剪后写入 `events.jsonl.compacted` 的显式标记行，保留 `seq` 空洞语义，
  SSE 重放逻辑不需要为压缩做特例。
- 导出：`arkd export --thread thr_x --format tar.zst` 直接打包目录；
  导入：`arkd import <archive>` 解包 + 重建索引。这就是"会话迁移"的产品化入口。

#### 7.4.5 与旧数据的兼容

- `import-jsonl` 子命令：读取旧 `data/threads/<id>/*.jsonl` 与
  `data/child-runs/<child>/*`，生成新布局（把 child 搬进父目录的 `children/`）。
- **迁移顺序是硬约束**（修复 codex 第二轮阻塞项）：

```text
1. 扫描旧 data/child-runs/，读出每条 ChildRunRecord（含 parent_thread_id）
2. 校验：parent_thread_id 指向的父目录存在；child_id 全局无重复
   → 任一校验失败：整批进入"待人工处理"清单，不写任何数据（不留半个迁移现场）
3. 先迁父会话（threads/<parent>/…）
4. 再迁子会话正文到 threads/<parent>/children/<child>/
5. 最后追加写 threads/<parent>/children/runs.jsonl（权威记录）
6. 重建 index.json 与全局索引
7. 只有以上全部成功，才执行 orphan cleanup（清理无父记录的残留目录）
```

  关键是 **orphan cleanup 必须在记录迁移完成之后**：否则会先删掉
  "暂时还没建立父记录"的合法子会话。
- 迁移是**幂等**的：重复执行不会重复搬移；搬移用"先复制 + 校验 + 再删旧"的顺序。
- 缺失记录、重复 `child_id`、父记录不存在 → 都是**显式迁移错误**，
  列在报告里并有非零退出码，不再"跳过继续"。
- 迁移完成后写 `data/.migrated-from-v1` 标记，便于回退时识别。

数据库选型说明：索引用 `rusqlite`（bundled SQLite）足够；
文件读写用 `tokio::fs`，actor 内部的同步写放 `spawn_blocking`，
不引入 async SQLite 驱动。

#### 7.4.6 全局事件与审计主干（分片、分级，不设全局串行瓶颈）

全局总线只负责路由，**不重新持久化每个线程事件，也不设置一把全局锁**：

```text
ThreadWriter(shard by thread_id) ── durable event ──► EventRouter(shard N)
Plugin/Runtime operation ────────── audit command ─► AuditWriter(single durable writer)
EventRouter ─► bounded subscriber channels / metrics aggregator / SSE adapters
```

- **事件路由分片**：按 `thread_id`（无线程事件按 `plugin_id`）一致性哈希到固定数量的
  `EventRouter` actor。每个分片有界队列，保持同一线程顺序；不同线程并行。
  事件只有在线程 writer `fsync` 成功后才发布，因此路由器宕掉最多影响实时通知，
  客户端可从真源按 `since_seq` 重放。
- **订阅者隔离**：每个订阅者独立有界 channel；慢订阅者断开或收到可恢复的 gap，
  不阻塞路由分片、线程 writer 或其它订阅者。全局诊断只消费聚合计数和采样事件，
  不复制完整消息正文。
- **审计分级**：`SecurityCritical`（授权、审批、凭据、删除、插件安装）必须写入
  独立 append-only `data/audit/audit.jsonl` 并收到 `fsync` ack 后操作才算成功；
  审计不可用时此类操作 **fail closed**。`Operational` 审计可有界缓冲、批量写，
  但队列满时返回显式降级状态；普通 trace/log 可采样或丢弃，三者不得共用一个阻塞队列。
- **唯一写者**：`AuditWriter` 是全局审计文件的唯一写者，批量上限 128 条或 10 ms；
  日志轮转采用“落盘 → fsync → 原子改名”。审计事件带宿主注入的 `plugin_id`、调用者、
  `trace_id`、风险级别和脱敏参数摘要，插件不能覆盖这些字段。
- **资源预算**：路由分片数、队列容量、单订阅者容量均由启动配置给出并设硬上限；
  暴露队列深度、拒绝/断连数、审计 fsync p99。过载策略必须可压测，禁止无界 channel。

### 7.5 ark 作为能力总线（插件不各自造通信层）

**问题**：如果每个插件各自建 HTTP 客户端、各自起 MCP server，会立刻复现现状的老毛病——
代理/证书/重试/超时各写一遍（§2.1 C-6 的翻版）、凭据散落在各插件、
每个插件开一个监听端口、插件之间互相调用还要知道对方的地址与协议版本。

**设计**：ark 统一提供三组**基础能力**（HTTP 出站、MCP 开放与互调、界面承载），
插件只声明"我要用"，不自己实现通信层，也不各自开端口或窗口。

| 基础能力 | ark 提供 | 插件得到 | 插件**不得**自己做 |
|---|---|---|---|
| HTTP 出站 | `ark-http` | 统一代理、TLS、超时、重试退避、限流、重定向策略、`trace_id` 透传、凭据注入、响应脱敏 | 自建 `reqwest::Client`、自管代理与证书 |
| MCP 开放 | `ark-mcp` 出口 | 把工具注册进 ark 的统一 MCP 出口（传输与握手由 ark 负责） | 自己起 MCP server、自己管端口 |
| MCP 调用 | `ark-mcp` 总线 | 按 `PluginId` 调用**其它插件已注册的 MCP 能力** | 直连对方进程或地址 |
| 界面承载 | `ark-ui` 总线 | 把页面/面板注册进 ark 的界面容器，复用路由、鉴权、主题、错误边界、加载态 | 自己起窗口、自己管路由与登录态 |

一句话：**插件只写领域逻辑与页面，通信、治理、界面承载由 ark 兜底。**

#### 7.5.1 HTTP 基础能力

```rust
#[async_trait]
pub trait HttpCapability: Send + Sync {
    async fn request(&self, req: OutboundRequest) -> Result<OutboundResponse>;
    /// 插件只引用凭据名，由 ark 解析并注入；插件拿不到明文
    fn credential(&self, name: &CredentialName) -> Result<InjectedHeader>;
}

pub struct OutboundRequest {
    pub method: Method,
    pub url: Url,                      // 只能落在 manifest 声明的 allowlist 域内
    pub headers: HeaderMap,            // ark 追加 Authorization / trace 头
    pub body: Body,
    pub timeout: Option<Duration>,     // 缺省取 manifest 的网络策略
    pub idempotency_key: Option<String>,
}
```

约束：

- **域名 allowlist**：`manifest.http.allow = ["api.example.com", "*.corp.example"]`，
  越界请求直接拒绝并记 `http_denied` 诊断，避免插件变成任意 SSRF 跳板。
- **凭据不落地插件**：明文存 OS keyring / `data/secrets`，
  ark 在发请求时注入；日志与事件只记凭据名与哈希前缀。
- **统一可观测**：每个出站请求落 `http_span`（`plugin_id`、脱敏 URL、状态码、
  耗时、重试次数），与 agent 的 `llm_span` 同一套 tracing / OTLP。
- **统一限流与熔断**：按 `plugin_id + host` 维度限流，单插件故障不拖垮全局。
- **共享缓存**：`GET` 可按 manifest 声明走 ark 的 TTL 缓存，避免每个插件各建一层。

#### 7.5.2 MCP 基础能力（开放 + 调用）

插件**不启动自己的 MCP server**，而是把工具注册进 ark 的统一出口：

```rust
#[async_trait]
pub trait McpCapability: Send + Sync {
    /// 插件开放自己的能力；返回句柄，Drop 时自动下线
    fn expose(&self, tools: Vec<McpToolSpec>) -> Result<ExposureHandle>;
    /// 插件调用其它已注册能力：进程内路由，不走网络与 JSON-RPC 序列化
    async fn call(&self, provider: &PluginId, tool: &str, args: Value) -> Result<McpCallResult>;
    /// 当前总线上的全部能力（自描述）；用于插件发现彼此
    fn catalog(&self) -> McpCatalog;
}
```

- **一个出口**：所有插件贡献的 MCP 工具合并成一份 catalog，
  由 ark 统一以 stdio / streamable-http 暴露；外部客户端只需连 ark 一个端点，
  不需要知道背后有几个插件、各自部署在哪。
- **插件互调走总线**：`call(provider, tool, args)` 在进程内直接路由到目标插件的 handler，
  带调用链 `trace_id`、深度限制（默认 3 层）与循环检测，不经过网络。
- **命名空间**：工具名统一为 `<plugin_id>.<tool>`（如 `ark.builtin.studio.run_case`），
  冲突在注册时失败，而不是运行期静默覆盖。
- **显式可见性**：manifest 的 `mcp.expose` 声明对外暴露哪些工具，
  `mcp.consume` 声明依赖哪些其它插件的工具；未声明的调用被拒绝。
  依赖因此可审计，也能用于启动时拓扑排序。
- **单一自描述来源**：`catalog()` 同时喂给 `GET /v1/runtime/capabilities` 与
  MCP 的 `tools/list`，不存在两份清单。

#### 7.5.3 界面基础能力（Code 模式界面本身也是插件）

**Code 模式界面不再硬编码进外壳**，它是 `ark-ui` 总线上的一个插件
（`ark.builtin.code-ui`）；其它插件用同一套 API 注册自己的页面，因此
"新增一个业务面板"和"启用/停用 Code 模式"是同一种操作。

```rust
#[async_trait]
pub trait UiCapability: Send + Sync {
    /// 注册一个页面：路由、导航位置、图标、所需能力
    fn register_page(&self, page: PageSpec) -> Result<PageHandle>;
    /// 注册一个可在页面内嵌的面板（右栏/底栏/侧栏）
    fn register_panel(&self, slot: PanelSlot, panel: PanelSpec) -> Result<PanelHandle>;
    /// 页面可声明自己需要哪些后端/工具能力，宿主据此做门禁
    fn require(&self, caps: &[Capability]) -> Result<()>;
}

pub struct PageSpec {
    pub id: PageId,                    // "ark.builtin.code"，或 "ark.example.jira.board"
    pub route: String,                 // "/code"、"/jira"
    pub title: I18nKey,
    pub icon: IconRef,
    pub placement: Placement,          // PrimaryNav | SecondaryNav | Settings
    pub bundle: AssetRef,              // 前端产物（内置=编译进 bundle，外部=manifest 声明的资源）
    pub required_caps: Vec<Capability>,
}
```

Ark 统一提供、插件不得自建的界面基础能力：

| 能力 | 说明 |
|---|---|
| 路由与导航 | 一个容器、一套路由表；插件只声明 route，不接管 history |
| 登录态与鉴权 | 复用 ark 的 session/token；页面不自己存 token |
| 主题与设计系统 | 统一 token（颜色/间距/字体），插件不得自带一套样式基线 |
| 错误边界与降级 | 单个页面崩溃不拖垮整个应用，展示可恢复的错误页 |
| 加载/空态/骨架屏 | 统一组件，避免每个插件各写一套 |
| 权限门禁 | 按 `required_caps` 与当前后端能力隐藏/禁用入口 |
| 事件桥 | 页面订阅 ark 事件（回合、工具、运行状态）走同一通道，不各自开 SSE |

约束：

- **Code 模式是插件，不是特权**：`ark.builtin.code-ui` 停用后应用仍能启动，
  只剩导航骨架，证明界面确实由总线驱动。
- **页面不直接访问运行时**：只能经 ark 提供的 typed client（由 `ark-contract` 生成），
  不允许 `fetch('/v1/...')` 直连，便于统一鉴权与埋点。
- **外部插件页面默认沙箱化**：运行在独立 WebView / sandbox iframe，
  只经 `postMessage` 走受限 API；内置插件页面在同一 bundle 内，可用完整 typed client。
- **懒加载**：页面 bundle 按路由分包，未访问的插件页不进入首屏。

#### 7.5.4 平台服务插件：基础设施的发布形态

**三层模型**（与 §7.5.1–7.5.3 的关系，先说清楚避免"两层插件"的混淆）：

| 层 | 内容 | 形态 | 能否被业务插件替换 |
|---|---|---|---|
| 能力端口 | `HttpCapability` / `McpCapability` / `UiCapability` 等 trait，外加身份、授权、配额、审计 | 编译进 arkd | **否**（安全边界） |
| 平台服务插件 | 日志、HTTP 实现、凭据、配置视图、KV、文件、i18n、通知 | **ark 官方维护、可独立版本化**，v1 静态装配进 arkd | 实现可换，端口不可换 |
| 业务插件 | Studio、后端集成、IM、第三方能力 | 内置或外部（MCP） | — |

首批平台服务（v1）：

| 服务 | 职责 | 多插件隔离要点 |
|---|---|---|
| `ark.log` | 结构化日志、脱敏、落盘/转发 | 每条自动带 `plugin`、版本、`trace_id`；逐插件限流 |
| `ark.http` | §7.5.1 的出站 HTTP（首个服务样板） | allowlist / UA / 限流 / 熔断按 `plugin_id` 分区 |
| `ark.credentials` | 凭据保管与调用期注入 | 属主为 `plugin_id`；只发 opaque handle |
| `ark.config` | 插件私有配置视图 + 热更新 | 命名空间 `{plugin_id}.{key}`；schema 校验 |
| `ark.kv` | 插件私有小状态 | 键空间固定归属；配额与保留策略 |
| `ark.files` | 插件私有文件/附件 | 根目录 `data/plugins/<plugin_id>/`；拒绝穿越与链接逃逸 |
| `ark.i18n` / `ark.notify` | 国际化资源 / 通知路由 | **可延后**：出现 ≥2 个真实消费者再做 |

**内核最小集（绝不能插件化，否则无法自举）**：配置引导与迁移入口、stderr 启动日志、
`plugin_id` 身份与 grant、manifest 解析与依赖拓扑、权限/审计/事件主干、
进程监督与资源配额、HTTP/MCP 的域名与调用边界。
任一服务插件缺失或损坏时，`arkd` 以**最小诊断模式**启动，依赖它的插件 fail closed 跳过，
**不得**因自举失败而绕过授权。

**依赖与版本**：`requires` 从裸 `PluginId` 升级为带 semver 的服务依赖：

```json
{ "requires": { "ark.log": "^2", "ark.http": "^1", "ark.builtin.studio": "^1" } }
```

- 解析结果写入 lock 清单，离线安装可复现；主版本不兼容 → 依赖插件不启动并给诊断。
- 循环依赖、重复服务、缺失依赖都在**启动前**失败，不做运行期兜底。
- 依赖方向固定：业务插件 → 服务端口 → 内核；服务实现只依赖内核端口与基础 crate。

**外部插件的同等服务**：外部（MCP）插件没有 Rust `PluginContext`，
它通过 **`ark.host` 服务目录**（一组受限 MCP 工具：`ark.host.log` / `ark.host.config` /
`ark.host.http` / `ark.host.kv` / `ark.host.files`）获得同样的基础设施，
调用时带插件专属短期身份与 grant。**服务一样、隔离一样，只是通道不同**；
不因此获得任意数据库、路径或明文密钥访问。

#### 7.5.5 多插件隔离约定（默认强制，manifest 不可关闭）

`plugin_id` 是宿主注入的稳定机器身份（如 `ark.builtin.im`），
展示名不参与命名空间；插件**不能**传入另一个 `plugin_id` 冒充自己。

| 范围 | 强制约定 | 校验手段 |
|---|---|---|
| 身份 | 上下文由宿主注入 `plugin_id` / 版本 / grant | 编译期私有构造 `PluginIdentity`；运行期令牌校验 |
| 日志 | 每条自动带 `plugin=<plugin_id>`、版本、`trace_id`；**不可覆盖**，统一脱敏 | 类型化 `Logger` 绑定身份；lint 禁全局 logger |
| 配置 | 逻辑键 `{plugin_id}.{key}`（如 `ark.builtin.im.route.*`）；物理按插件分区 | `ConfigView<T>` 限定 owner；读写归属校验 |
| 事件 | 插件主题 `plugin.<plugin_id>.<topic>`；订阅他人/全局事件需 grant | 主题生成校验；运行期 ACL 与保留量检查 |
| 存储路径 | 私有根固定 `data/plugins/<plugin_id>/`；只给相对句柄 | `StorageHandle`；拒绝路径穿越与符号链接逃逸 |
| 凭据 | 引用名 `{plugin_id}/{name}`；插件只持有 handle | `CredentialRef` 无明文读取 API；调用期注入 |
| 缓存 key | 自动加 `plugin/<plugin_id>/` 前缀；租户间不命中 | `CacheHandle` 绑定身份；配额与逐插件清空 |
| MCP 工具名 | `<plugin_id>.<tool>`；冲突注册失败 | 注册 API 强制；catalog 查重 |
| HTTP 标识 | `User-Agent: ark-plugin/<plugin_id>@<version>` 受保护，不可覆盖 | `OutboundRequest` 不暴露保留头写入口 |
| 错误码 | `plugin.<plugin_id>.<domain>.<code>`，不含 token/路径/堆栈 | newtype 生成；lint 查重与格式校验 |
| 指标 | 宿主注入 `plugin_id` 维度；标签白名单 + 基数上限 | `MetricHandle` 绑定身份；未知标签拒绝 |
| 通知 | 路由固定含 `plugin_id`；用户可逐插件关闭 | `NotificationHandle` + manifest 声明 |

> **不可弱化的边界**：默认句柄 ≠ 默认全量权限。
> 私有日志 / 配置 / KV 是安全默认；HTTP、通知、凭据、全局事件订阅仍需
> manifest 声明 + grant。**隔离默认强制，不提供关闭开关。**

#### 7.5.6 这层总线解决了什么

1. **换后端只加插件**：插件只写领域逻辑（用例、执行、制品），
   通信、鉴权、重试、观测全部复用 ark，代码量与出错面显著缩小。
2. **插件可组合**：Studio 插件可通过总线调用 Jira 插件查缺陷、
   调用内部平台插件上报结果，彼此不需要知道对方的部署位置与协议版本。
3. **治理集中**：域名 allowlist、凭据、限流、审计只在 ark 一处，
   不随插件数量增长而失控。
4. **界面可组合**：任何插件都能把自己的页面挂进同一个导航，
   复用登录态、主题、错误边界与事件桥；Code 模式与第三方面板地位相同。

---

### 7.6 MCP：双向能力（总线的一个投影）

> 本节是 §7.5.2「MCP 基础能力」的展开。**插件不自己起 MCP server**，
> 只把工具注册进 ark 的统一出口；外部客户端连的是 ark，不是插件。

**现状**：只有客户端方向。`kun/src/adapters/tool/mcp-tool-provider.ts`（约 31KB）实现了
stdio / SSE / streamable-http 三种传输、`tools/list` 分页、`tools/call`、
trustScope、名称冲突消解、schema 搜索索引。这套能力**继续复用**，不重写。

**缺口**：agent 无法在会话里操作本应用。当前每加一个"让 agent 能做什么"的需求，
就要在 `runtime-factory.ts` 里手工加一个专用工具，并单独维护一份 schema。
这既是 §2.3 巨型装配的一部分，也让 MCP 生态无法复用。

**新设计**：MCP 双向。客户端保留，**新增服务端**。

#### 7.6.1 客户端（保留并收敛）

```rust
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse>;
    async fn notify(&self, n: JsonRpcNotification) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

// 三个实现：StdioTransport / SseTransport / StreamableHttpTransport
```

- **出站请求走 `ark-http`**：MCP 的 SSE / streamable-http 传输复用同一条出站管线
  （代理、TLS、超时、重试、限流、凭据注入），不另建 `reqwest::Client`。
- 连接生命周期由 `McpClientRegistry` 统一管理（连接池、重连退避、超时）。
- 工具命名冲突、`trustScope`、`tool_search` 索引**对齐现状语义**，
  便于新旧运行时差分对比。
- 外部 MCP 工具进入 `ToolCatalog` 的方式与内置工具**完全一致**，
  由插件注册表统一收集（见 §7.7）。

#### 7.6.2 服务端（新增）

ark 提供**唯一** MCP 出口，所有插件（含内置）把自己的工具注册进来：

```rust
// crates/ark-mcp/src/server/ —— 由 ark 持有，插件不实例化
pub fn serve(bus: Arc<dyn McpCapability>) -> McpServer
```

- 内部 agent 走进程内 adapter（不经协议）；外部客户端走 stdio / streamable-http。
- 插件贡献的工具与 ark 内置工具（线程/回合/审批/用量）合并成**一份 catalog**，
  名字统一为 `<plugin_id>.<tool>`。
- 插件之间互调走 `McpCapability::call(provider, tool, args)`，
  进程内路由，不经过网络，也不使用对方的地址或端口。

暴露的能力清单（第一版，按"agent 真正常用"排序）：

| 领域 | 工具名 | 作用 | 幂等 |
|---|---|---|---|
| 线程 | `ark_thread_list` / `ark_thread_get` | 列出/读取会话 | 是 |
| 线程 | `ark_thread_create` / `ark_thread_update` | 新建/改名/归档 | 否/是 |
| 回合 | `ark_turn_start` | 在指定线程发起新回合 | 否 |
| 回合 | `ark_turn_steer` / `ark_turn_interrupt` | 介入/中止当前回合 | 否 |
| 事件 | `ark_events_replay` | 从 `since_seq` 拉事件 | 是 |
| 闸门 | `ark_approval_resolve` | 回应审批请求 | 否 |
| 用量 | `ark_usage_summary` | 读线程/全局用量与缓存命中 | 是 |
| 诊断 | `ark_runtime_info` / `ark_tool_list` | 读运行时与工具诊断 | 是 |
| Studio | `ark_studio_case_list` / `ark_studio_run` | 列用例 / 触发一次执行 | 是/否 |
| Studio | `ark_studio_artifacts` | 列某次运行的制品 | 是 |

**拓扑（先定清楚，否则无法验收"会话内直接操作"）：**

```text
     ┌──────────────── ark 进程 ────────────────┐
     │  AgentLoop ──tools──► InternalAppTools   │  ← 内部 agent 走这条（进程内调用）
     │                          │               │
     │                          ▼               │
     │                    AppCapabilityService  │  ← 与 HTTP 共用同一层 service
     │                          ▲               │
     │  ark-server(axum) ───────┤               │
     │  McpServer(stdio/http) ──┘               │
     └──────────────────────────────────────────┘
              ▲                    ▲
              │                    │
        外部 MCP 客户端        外部 MCP 客户端（远程）
```

- **内部 agent 不经过 MCP 协议连自己**：它调用 `InternalAppTools`（进程内 adapter），
  与 MCP server 共享同一个 `AppCapabilityService`。
  这既避免了自调用环路的传输开销，也让"同一能力只有一份实现"成立。
- **外部 MCP server 是另一层 adapter**，只做协议翻译 + 鉴权 + 作用域校验。
- stdio 模式必须是**独立进程** `arkd mcp serve`（不与主进程争 stdin/stdout），
  由客户端按需拉起；主进程只保留 streamable-http（loopback + token）。

设计约束：

- **与 HTTP 路由共用同一层 service**：MCP 工具只是另一层 adapter，
  不允许出现"MCP 有一套业务逻辑、HTTP 有另一套"。
  编译期强制：`ark-mcp` 的 handler 只能依赖 `ark-app::service` 的 trait，不能直接碰 store。
- **写操作必须有作用域**（区分创建与操作已有线程）：
  - 对**已有**线程的写（`turn_start`/`steer`/`interrupt`/`approval_resolve`）强制 `thread_id`；
  - `ark_thread_create` 没有既有 `thread_id`，改为强制 `workspace_id` + 显式
    `parent_thread_id: Option`，并在校验层确认调用方对该 workspace 有写权限。
  - 内部 agent 调用 `turn_start` 时，**禁止指向自己当前正在执行的线程**（否则形成递归），
    并对新线程设置 `max_depth`（复用子代理预算）与并发上限。
- **调用者身份（token 只是认证，身份必须可追踪）**：
  每个 MCP 客户端在首次连接时注册为 `McpClientIdentity { client_id, name, pid, transport }`，
  由用户在 GUI 里一次性授权，签发 `ClientGrant`：

```rust
pub struct ClientGrant {
    pub client_id: String,
    pub allowed_scopes: Vec<Scope>,     // Thread(id) / Workspace(id) / Global
    pub allowed_tools: Vec<ToolName>,   // 白名单，默认空
    pub max_risk: RiskLevel,            // Read < Write < Sensitive
    pub expires_at: Option<DateTime<Utc>>,
}
```

- `ClientGrant` 存在 `data/grants.json`（原子写），撤销立即生效；
  未授权客户端只能看到 `tools/list` 的公开子集。
- **能力授权分级**：

| 级别 | 典型工具 | 要求 |
|---|---|---|
| 只读 | `thread_list`、`events_replay`、`usage_summary` | 有效 token + grant 含对应 scope |
| 写入 | `turn_start`、`steer`、`studio_run` | token + scope + `max_risk >= Write` + 审计 |
| 敏感 | `approval_resolve`、`interrupt`、删除类 | 上述 + **宿主侧确认**：由 `arkd` 发 `approval_requested` 事件，等 GUI 或 CLI 显式返回决议后才执行 |

- **`confirm: true` 参数不能自证**：调用方自己传 `confirm: true` 不算用户确认。
  敏感操作只能走宿主发起的审批流（复用现有 `approvalGate`），
  MCP 侧拿到的是审批结果（`approval_id` → 已决议），而不是一个布尔参数。

- **审计与脱敏**：写调用落 `tool_call_started/finished` 事件并带
  `source: "mcp_server"`；参数按 `ark_common::redact` 脱敏后入库，
  敏感值（口令、token）只记录长度与哈希前缀。
- **幂等性显式声明**：MCP `annotations.idempotentHint` 与内部 `Tool::idempotent()` 同源，
  避免同一能力在两处各写一遍。
- **不递归**：暴露的工具列表里**不包含**任何"MCP 调用 MCP"的反射入口
  （无 `ark_mcp_call` / `ark_tool_invoke_any`）；工具白名单静态生成，
  不接受运行期任意注册。

#### 7.6.3 传输与开关

- stdio（本地子进程，默认）+ streamable-http（远程/多客户端）。
- 默认**仅监听 loopback + 复用 runtime token 鉴权**，与 HTTP 边界同一套认证；
  远程模式必须显式开启并配 TLS/反向代理，配置项名沿用 `mcpServer.*`。
- 工具可见性受 `trustScope` 控制，与客户端方向复用同一套策略类型。

---

### 7.7 一切皆插件

**目标**：新增功能（一个工具、一个模型 provider、一个 MCP 连接、一个 Studio 动作、
一个界面面板）应当是"加一个插件目录 + 注册一行"，而不是改 `runtime-factory.ts`。

#### 7.7.1 插件模型

```rust
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    /// 向宿主注册本插件贡献的能力；返回注册句柄，`Drop` 时自动反注册。
    fn register(&self, ctx: &mut PluginContext) -> Result<Registration>;
}

pub struct PluginManifest {
    pub id: PluginId,                 // 稳定唯一标识，如 "ark.builtin.bash"
    pub version: semver::Version,
    pub kind: PluginKind,
    pub capabilities: Vec<Capability>,
    pub config_schema: Option<JsonSchema>,  // 插件自己的配置约束
}

pub enum PluginKind {
    Tool,
    ModelProvider,
    McpClient,
    StudioAction,
    UiPage,       // ← 注册页面（Code 模式自己也走这条）
    UiPanel,      // ← 只注册一个可嵌入面板，不占导航位
    Backend,      // ← 后端集成，与其它能力同一套契约（§7.12）
}
```

**插件拿到的只有基础能力，没有特权**（对应 §7.5）：

```rust
pub struct PluginContext {
    /// 宿主注入的稳定身份；插件不能自报，也不能换成别的 plugin_id
    identity: PluginIdentity,        // { plugin_id, version, grant }

    // ---- 三组基础能力（§7.5）----
    http: Arc<dyn HttpCapability>,   // 出站 HTTP：代理/TLS/重试/限流/凭据注入
    mcp:  Arc<dyn McpCapability>,    // MCP 出口 + 总线：开放自己的工具、调用别人的工具
    ui:   Arc<dyn UiCapability>,     // 界面承载：注册页面/面板、复用路由与主题

    // ---- 平台服务句柄（§7.5.4，均已绑定 identity）----
    log:   Arc<dyn LogService>,      // 每条自动带 plugin=<plugin_id>
    cfg:   ConfigView,               // 私有配置视图：{plugin_id}.{key}
    cred:  Arc<dyn CredentialService>, // 只发 handle，拿不到明文
    kv:    Arc<dyn KvService>,       // 键空间固定归属本插件
    files: Arc<dyn FileService>,     // 根目录 data/plugins/<plugin_id>/
    trace: Arc<dyn TraceService>,    // span 自动带 plugin_id
    metrics: Arc<dyn MetricService>, // 维度自动注入 plugin_id
    notify:  Arc<dyn NotifyService>, // 路由固定含 plugin_id

    // ---- 可选能力（manifest 声明 + grant 后才有）----
    events: Option<Arc<dyn EventBus>>,  // 订阅他人/全局事件需 grant
}
```

**所有句柄都由宿主构造并绑定 `identity`**：插件拿到的 `log` 已经知道自己是哪个插件，
`cfg` 只能读写自己的命名空间，`kv` / `files` 的根目录已被限定。
插件**无法**通过传参切换到别人的命名空间——这正是"默认多插件共用"的前提。

约束：

- 插件**不得**自建 `reqwest::Client`、不得自起 MCP server、不得自己开窗口或路由。
  这三条由 `xtask lint-deps` + 依赖图检查强制（插件 crate 不允许依赖 `reqwest`/`axum` 等）。
- 插件之间的调用**只能**经 `ctx.mcp.call(provider, tool, args)`，
  不允许直连对方地址或共享内存。

注册点（`PluginContext` 上的显式方法）：

| 注册方法 | 贡献内容 | 与现状对应 |
|---|---|---|
| `register_tool` | 一个 `Tool` | `builtin-tools.ts` 的 10 个内置工具 |
| `register_model_provider` | 一个 `ModelClient` 分支 | `compat-model-client` 的三种 `endpointFormat` |
| `register_mcp_client` | 一个 MCP 连接 | `mcp-tool-provider.ts` |
| `register_studio_action` | 一个 Studio 步骤执行器 | `packages/runner` 的 `StepAction` |
| `register_page` | 一个页面（路由/导航位/图标/bundle/所需能力） | Code 模式主界面、Studio 页；当前外壳里的硬编码路由 |
| `register_panel` | 一个可嵌入面板（右栏/底栏/侧栏槽位） | Changes / Plan / File / Terminal |
| `register_backend_provider` | 一个 `BackendProvider` 实现 | 现状写死的 Galaxy 直连（§7.12） |

所有注册项都进入同一份 **capability catalog**，同时喂给：
`GET /v1/runtime/capabilities`、MCP `tools/list`、前端导航表。
因此"插件注册了什么"只有一处事实，不存在三份清单各自漂移。

> `register_backend_provider` 接收的是 **`dyn BackendProvider`（端口实现）**，
> 端口 trait 定义在 `ark-ports`，`ark-studio` 只依赖端口；
> 具体实现（local / galaxy / 第三方）全部在 `plugins/plugin-backend-*` 里。
> 因此 `PluginKind::Backend` 与其它 kind 走完全相同的
> manifest → registry → scaffold → discovery 流程（不再有第二套机制）。

#### 7.7.2 两类插件，同一条契约

| 类型 | 形态 | 注册时机 | 能力边界 |
|---|---|---|---|
| **内置插件** | 编译进 `ark-plugin` 的 crate（`plugin_bash`、`plugin_mcp`、`plugin_code_ui`…） | 编译期 `inventory` 收集，启动时遍历 | 全能力，可直接依赖 `ark-tool`/`ark-llm`/`ark-ui` |
| **外部插件** | 独立进程，经 **MCP** 接入；或 WASM（后续） | 启动时读 `plugins/*/manifest.json` | 只经 MCP 协议，无进程内特权 |

关键决策：**外部插件走 MCP，不发明第二套插件协议。**
好处是插件作者只需要会 MCP；坏处是插件不能直接改内存状态——
但这也正是我们想要的隔离。

#### 7.7.3 插件清单与发现

```text
data/plugins/
└── ark.example.jira/
    ├── manifest.json      # id / version / kind / capabilities / configSchema
    ├── config.json        # 用户配置（schema 校验，敏感字段走 keyring）
    └── (可选) bin/        # 外部插件的可执行文件
```

- `manifest.json` 用 `schemars` 从 Rust 的 `PluginManifest` 生成，
  **不允许手写第二份 schema**（对齐 §6.1 的单一真源原则）。
- 加载失败（schema 不合法、版本不兼容、可执行文件缺失）→
  记一条 `plugin_load_failed` 诊断并**跳过该插件**，不影响其他插件启动。
  这条直接规避现状 S-2 那类"一个失败毒化全局"的问题。
- 插件启用状态、最后加载结果、贡献的能力列表进 `GET /v1/runtime/plugins`。

#### 7.7.4 脚手架与模板

```bash
cargo xtask new-plugin --kind tool  --id ark.example.jira --dir ../ark-plugin-jira
cargo xtask new-plugin --kind model --id ark.example.qwen
cargo xtask new-plugin --kind mcp   --id ark.example.linear
```

生成的模板包含：

- crate 骨架（`lib.rs` 只 `pub use` 一个 `Plugin` 实现）
- manifest 生成代码
- 一个仓库内可跑的集成测试（含"插件被正确注册/反注册"断言）
- README，说明能力边界与配置字段

`xtask` 同时提供 `cargo xtask plugin-check`：
校验 manifest 与代码一致、`Capability` 声明与实际注册项一致、无重复 `PluginId`。

#### 7.7.5 防止"插件化"退化成新的巨型装配

- `PluginContext` 是**唯一**注册入口，`arkd` 的 `wiring.rs` 只做
  `for p in plugins { p.register(&mut ctx)? }`，不允许出现 `if plugin_id == ...` 的分支。
- 插件之间的依赖只允许"声明式"，且带版本范围：

```json
{ "requires": { "ark.log": "^2", "ark.http": "^1", "ark.builtin.studio": "^1" } }
```

  由宿主在**启动前**解析拓扑并校验版本：循环依赖、缺失服务、主版本不兼容
  都直接失败并给出诊断，不做运行期兜底（§7.5.4）。
- 插件不得直接读全局配置，只能拿到 `PluginContext::config::<T>()` 返回的
  已校验强类型配置。

---

### 7.8 UI 自动化：ark 的一个插件，且只有一个 agent

#### 7.8.0 先解决"两个 agent"（修复 S-7）

**现状（已核实）**：UI 自动化里同时存在两套决策循环：

| 循环 | 位置 | 作用 |
|---|---|---|
| ① `ark`（原 `kun`）agent 回合 | `studio-native-bridge.ts` → `createScenarioOrchestrator` → `/v1/threads/{id}/turns` | 规划场景步骤、决定下一步工具 |
| ② 官方 Playwright Test Agent | `session-host.ts:255-257` 启动 `playwright run-test-mcp-server`，`generator`/`healer` 两个 role | 自己再跑一遍"观察 → 决策 → 调工具"循环 |
| ② 的模型来源 | `desktop-adapter.ts:19-52`：`createDesktopAgentModel` → `createStudioAgentConversation` → **回调 ① 的 HTTP 接口** | 于是 ② 的每一步决策都要向 ① 发一个回合 |

结果是嵌套循环：① 的回合里发起 ②，② 的每步决策又回到 ①。
超时预算叠加（`STUDIO_AGENT_ACTION_TIMEOUT_MS` 20s / 导航 30s / 回放 60s / MCP 外层 5min），
失败归因困难，且两份 system prompt 各自演化。

**决策：只保留 `ark` 一个 agent。** 二选一，不折中：

| 方案 | 结论 | 说明 |
|---|---|---|
| A. 用 Playwright Test Agent 当主 agent，`ark` 退出 UI 自动化 | 不选 | 它是通用测试生成器，无法复用 `ark` 的审批、会话、usage、子代理、Code 模式上下文；也违背"一个 agent"目标 |
| B. 用 `ark` 当唯一 agent，Playwright 只做"手和眼" | **选定** | `ark` 负责全部决策；Playwright sidecar 只暴露确定性的浏览器原语 |

**方案 B 的形态**：

- 删除 `run-test-mcp-server` 这条"第二个 agent"通路，以及 `generator`/`healer` 的 role 概念。
- sidecar 只保留**确定性原语**（与现有 `RunnerCommand` 对齐）：
  `startBrowser` / `focusBrowser` / `navigate` / `observe` / `verifyLocator` /
  `verifyLocators` / `executeStepAction` / `executeBundle` / `pause` / `resume` / `cancel` / `shutdown`。
- `observe` 返回**结构化页面快照**（可访问性树裁剪 + 候选 locator + 可见文本），
  由 `ark` 的模型据此决策；sidecar 不做任何"决策"或"重试策略"。
- 用例生成 = `ark` 回合中的工具调用序列；修复 = 同一 agent 拿到失败证据后的下一轮，
  不是另一个 agent 的自循环。
- 超时预算只有一层：由 `ark` 的回合预算 + 单工具超时构成，不再有外层 MCP 与内层 agent 的叠加。

收益：一个模型上下文、一份 system prefix（对 prompt cache 有利）、
一套审批/中断/usage、一条可差分的执行轨迹。

迁移代价：需要把现有 `generator`/`healer` 的 prompt 资产**转成 `ark` 的工具说明或 skill**，
而不是转成第二套 agent。这一步单列为里程碑 M7 的门槛，不允许"先并行跑着再说"。

#### 7.8.1 UI 自动化本身是一个插件

UI 自动化不进 `ark` 核心，而是实现为一个 `Plugin`（§7.7）：

```rust
pub struct StudioPlugin { manifest: PluginManifest }

impl Plugin for StudioPlugin {
    fn register(&self, ctx: &mut PluginContext) -> Result<Registration> {
        // 1) 向 agent 暴露工具（由 ark 的同一个循环调用）
        ctx.register_tool(Box::new(StudioCreateCaseTool::new(ctx.port::<dyn CaseBackend>()?)));
        ctx.register_tool(Box::new(StudioGenerateStepsTool::new(ctx.port::<dyn StudioPlanning>()?)));
        ctx.register_tool(Box::new(StudioRunCaseTool::new(ctx.port::<dyn BrowserDriver>()?)));
        ctx.register_tool(Box::new(StudioReadArtifactsTool::new(ctx.port::<dyn ArtifactStore>()?)));
        // 2) 步骤执行器（IR → 浏览器动作），供 executeBundle 使用
        ctx.register_studio_action(Box::new(NavigateAction));
        ctx.register_studio_action(Box::new(ClickAction));
        ctx.register_studio_action(Box::new(FillAction));
        ctx.register_studio_action(Box::new(AssertVisibleAction));
        ctx.register_studio_action(Box::new(AssertTextAction));
        ctx.register_studio_action(Box::new(AssertUrlAction));
        // 3) 前端页面（渲染层按注册表挂载，不再硬编码路由）
        ctx.register_page(PageSpec {
            id: "ark.builtin.studio".into(),
            route: "/studio".into(),
            placement: Placement::PrimaryNav,
            required_caps: vec![Capability::Studio],
            ..Default::default()
        });
        Ok(ctx.take_registration())
    }
}
```

**`StudioPlanning` 的边界（单 agent 保证的关键）：**

```rust
/// 纯确定性转换：步骤文本 + 页面观测 → StepPlan（IR）。
/// 实现内部**禁止**持有 ModelClient、禁止发起模型请求、禁止自建事件循环。
pub trait StudioPlanning: Send + Sync {
    fn plan(&self, input: PlanningInput) -> Result<StepPlan>;
}
```

- 需要模型参与的部分（例如"这句话对应哪个操作"）**只能**通过
  `StudioGenerateStepsTool` 在 `ark` 的当前回合内完成——即：工具被调用 →
  返回结构化结果 → 由同一回合继续。
- CI 断言：`ark-studio` 与 `plugin-studio` 的依赖里不得出现 `ark-llm`；
  且不得出现 `tokio::spawn` 起的独立决策循环。
  这条把"不会出现第二个 agent"从口头承诺变成编译期 + 静态检查约束。

约束：

- 关掉 `StudioPlugin` = 关掉整个 UI 自动化能力，`ark` 核心与 Code 模式不受影响。
  这条同时是验收用例：**未安装 Studio 插件时进程必须正常启动**。
- 插件只依赖 `ark-studio` 的领域类型与 `ark-contract` 的端口 trait；
  不允许直接依赖 `ark-server`、`ark-store` 的具体实现。
- `ark-studio` 保留为**领域 crate**（用例 IR、RunBundle、门禁、执行编排），
  但它是被插件使用的库，而不是组合根里的硬编码分支。

#### 7.8.2 浏览器边界决策

现有链路是"主进程 ⇄ runner 子进程 ⇄ Playwright CLI"。
**保留 Node sidecar，只重写编排层。**

| 方案 | 结论 | 理由 |
|---|---|---|
| A. Rust 全重写（chromiumoxide） | 不推荐为第一版 | 需重实现 locator 语义、自动等待、trace/video 录制；团队经验集中在 Playwright |
| B. 保留 Node sidecar + Rust 编排 | **推荐** | 复用已验证的浏览器能力，Rust 只接管协议、编排、制品、门禁 |
| C. 全部保留 TS | 不满足需求 | — |

```rust
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    async fn start(&self, headed: bool) -> Result<bool>;
    async fn navigate(&self, url: &Url) -> Result<()>;
    async fn observe(&self) -> Result<PageObservation>;   // 只观察，不决策
    async fn verify_locators(&self, c: &[LocatorCandidate]) -> Result<Vec<LocatorVerdict>>;
    async fn execute_step(&self, action: &StepAction) -> Result<()>;
    async fn execute_bundle(&self, b: &RunBundle, sink: &EventSink) -> Result<RunOutcome>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn cancel(&self) -> Result<()>;
}
```

`SidecarDriver` 是唯一实现；未来 `CdpDriver`（chromiumoxide）可并列加入，
插件与上层代码无需修改。

#### 7.8.3 子进程管理（直接修复 S-1）

```rust
pub struct RunnerProcess {
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _guard: ProcessGuard,   // Drop 时 kill + 清理临时目录
}
```

- 任何 `spawn` 都必须走统一构造器，构造器内部处理 `io::Error`，
  **返回 `Result` 而不是 panic**。
- 读 stdout 的循环把 `Err` 转成 `RunnerEvent::ProcessExited{code}` 事件，
  上层据此把运行标记为 `FAILED`，进程不崩。
- JSONL 协议边界做严格校验（现状已有 zod schema，Rust 侧用 `serde` + 版本字段）。

#### 7.8.4 门禁（直接修复 S-3 / S-6）

门禁从"可写字段"改为**由状态机产出的只读证据**：

```rust
pub struct GateEvidence {
    pub locators_verified: Verified<LocatorReport>,   // 只能由 LocatorVerifier 构造
    pub assertions_confirmed: Verified<AssertionReport>,
    pub typecheck_passed: Verified<TypecheckReport>,  // 必须来自真实 tsc --noEmit
    pub local_run_passed: Verified<RunOutcome>,
}
```

`Verified<T>` 用**私有字段 + 私有构造**（不是 `pub(crate)`），
仅验证器模块能产出，外部 crate 与同 crate 其它模块都无法伪造。

#### 7.8.5 制品与凭据（修复 S-4 / S-5）

- 制品目录用 `TempDir`（`tempfile` crate）持有，`Drop` 时回收；
  需要长期保留的制品显式 `persist()` 到应用数据目录。
- 保留策略：`max_age_days` + `max_total_bytes`，启动时与运行后各清理一次，
  但**跳过活动与暂停中的运行**。
- 变量快照：默认不落盘；必须落盘时用 `age`（或 OS keyring）加密，
  文件名与内容都做脱敏。审计日志写入失败绝不阻塞主流程（修复 S-2）。

#### 7.8.6 执行编排

```
ark-studio/
  ir/          # ActionIR、StepAction、断言
  bundle/      # RunBundle 构造、校验、落盘、版本迁移
  gate/        # 门禁证据（见 7.8.4）
  driver/      # BrowserDriver trait；SidecarDriver 实现放在 ark-plugin-studio
  artifact/    # 制品生命周期、上传、保留策略
  plan/        # 回归计划、变量管理、用例编排
```

每一步执行产生结构化 `RunnerEvent`（`runStarted` / `stepStarted` / `log` /
`stepFinished` / `artifactCreated` / `runFinished`），
与现有 `packages/contracts/src/runner-event.ts` 的 `runner-event/v1` 保持**字段兼容**，
便于新旧并跑对比。

### 7.9 HTTP / SSE 边界

```rust
// ark-server
Router {
    GET  /health                        // 免鉴权
    POST /v1/threads/{id}/turns
    GET  /v1/threads/{id}/events        // SSE
    // ... 与 §1.3 清单一一对应
}
```

设计要点：

- **路由表由单一 `routes!` 宏/表驱动生成**，鉴权作为中间件统一处理，
  避免现状"每条路由手写 `if (!authorize(...))`"（46 处重复）。
- SSE 用 `axum::response::sse`，每连接一个 `mpsc::Receiver<Event>`。
- **重放与订阅必须是同一个原子操作**（修复 codex 阻塞项 4）：

```rust
// 由同一个 writer actor 串行执行，中间不允许插入任何追加
pub struct SubscribeTicket {
    pub backlog: Vec<RuntimeEvent>,   // (since_seq, N] 的已落盘事件
    pub live: mpsc::Receiver<RuntimeEvent>,  // 只接收 seq > N
    pub watermark: u64,               // = N
}
impl SessionWriter {
    async fn subscribe(&self, thread: &ThreadId, since_seq: u64) -> Result<SubscribeTicket>;
}
```

  `subscribe` 在 actor 的同一临界区内完成"确定水位 N → 读 (since_seq, N] →
  注册只收 seq>N 的 channel"，因此不存在"读完快照、订阅建立前"的空窗。
  先发 backlog 再切 live，天然有序，客户端无需去重。
- **seq 的唯一权威是事件 writer**（落盘到 `events.jsonl` 的序号），
  不是 SQLite：索引里的 seq 只是拷贝（修正 v0.2 中"由数据库保证"的错误表述）。
- 慢客户端策略：**断开并让其用 `since_seq` 重连**，绝不阻塞 writer。
  `live` channel 有界（512），`send` 失败即关闭该连接；
  SSE 层额外发 `sse_idle_timeout` 提示，客户端带最后收到的 `seq` 重连。
- 压缩造成的 seq 空洞：`events.jsonl.compacted` 的标记行显式声明"某段 seq 已折叠"，
  `subscribe` 读到空洞时**直接跳过且不报错**，重放算法不为压缩做特例。
- 优雅停机：`/v1/shutdown` 触发 `CancellationToken`，
  等待所有活动回合落盘（带超时）后退出，退出码显式。
- 热更新：合并为**一条** `POST /v1/runtime/config`，
  携带 `ConfigPatch` 枚举（模型路由 / 子代理预算 / loop 调参 / 连接器环境），
  统一校验 + 统一版本号 + 统一回滚。消除 §2.6 的四套实现。
  生效方式（热更新 / 需重启）按字段分级，详见 §7.15。
- 连接器环境不再改 `process.env`：`ToolContext` 携带 `env: Arc<EnvSnapshot>`，
  工具执行时读取快照。这样是作用域明确的，也天然线程安全。

### 7.10 Code 模式界面（`ark-ui` 的一个插件）

> Code 模式界面的**页面与面板**由 `plugin-code-ui` 注册到 `ark-ui` 总线（§7.5.3），
> 与第三方插件页面地位相同；本节只讨论它自身的渲染架构。

#### 7.10.1 技术选型

推荐 **Tauri 2 + React/TS**：

- 保留 React 生态（流式 Markdown、虚拟滚动、diff、代码高亮都成熟）
- Electron 主进程那部分（窗口、托盘、更新、文件对话框、shell）由 Tauri/Rust 承担
- 安装包从 Electron 的百 MB 级降到十 MB 级
- IPC 从 `ipcRenderer.invoke` 换成 `@tauri-apps/api`，语义一一对应

不推荐纯 Rust GUI：`egui`/`iced` 在富文本流式渲染、文本选择、IME、无障碍上
需要大量自研，属于把工程风险从后端转移到更贵的前端。

#### 7.10.2 渲染层重构（修复 128KB 单体）

```
web/src/
  features/
    code-session/
      components/     # 会话流、消息气泡、工具卡片、审批卡片、用户输入
      hooks/          # use-turn-stream, use-thread-list, use-attachment
      state/          # zustand slice，按关注点拆分
      api/            # 只有 HTTP + SSE 客户端，无业务逻辑
    workbench/
      layout/         # 三栏布局、面板 resizer
      panels/         # Changes / Plan / File / Terminal
  shared/
    contracts/        # ← 从 ark-contract 生成，禁止手写
    ui/               # 基础组件（按钮、弹窗、Toast）
```

硬约束（进 CI）：

- 单文件 ≤ 400 行（可用 `eslint-max-lines` + `rustfmt`/`clippy` 对应规则）
- `features/*/api` 之外不得出现裸 `fetch`
- `shared/contracts` 目录为生成目录，CI 校验 git diff 为空
- 组件不得直接持有业务状态，状态在 `state/` slice
- **页面只能经 ark 的 typed client 访问运行时**，不允许直连 `/v1/*`
  （统一鉴权、埋点与降级；这也是外部插件页面能复用同一套桥的前提）
- **页面不得自带主题/弹窗/Toast 实现**，只能用 `shared/ui` 与 `ark-ui` 提供的基础组件

#### 7.10.3 事件映射

现状 `kun-mapper.ts` 58KB，负责把 SSE 事件转成渲染 item。
新设计把映射**下沉到 Rust**：服务端直接产出"渲染就绪"的 item 结构
（`TurnItem` 已是这个定位），渲染层只做分组与排序，不做语义推断。

这样做的收益：

- 映射规则只有一份，可单测（Rust 侧纯函数）
- 前端不再需要理解 46 种事件，只理解 `TurnItem` + 少量控制事件
- 现存重复的"Workbench 与 code-session-helpers 各有一份纯函数"问题不会再出现

### 7.11 可观测性

- `tracing` + `tracing-subscriber`，JSON 输出，字段与现状日志保持兼容命名
  （`thread_id`、`turn_id`、`model`、`provider`、`endpoint_format`、`request_url`）
- OTLP 导出沿用 `opentelemetry-otlp`，span 名对齐 `llm_span_start/end`
- usage 计量统一为 `UsageRecorder`，从事件流增量更新，
  重启后用索引聚合恢复累计值（对齐现状"从持久化 usage 事件恢复"的行为）

### 7.12 后端集成：把"写死智测方舟"变成插件

这是"剥离后端"最关键的一层。当前 Studio 链路把 Galaxy 的形状刻进了业务代码
（§2.7 S-8）。重写后，`ark` 只认识**一个后端契约**，具体平台是插件。

**后端插件不自己造通信层**：所有出站请求走 `ark-http`（代理/重试/限流/凭据注入/审计），
所有对其它插件的能力调用走 `ark-mcp` 总线。插件里只剩"平台字段 ↔ 领域模型"的映射。

#### 7.12.1 契约（`ark-ports::BackendProvider`，不含任何平台字段）

```rust
#[async_trait]
pub trait BackendProvider: Send + Sync {
    fn id(&self) -> &'static str;                    // "galaxy" / "local-files" / "jira-xray"
    fn display_name(&self) -> &str;

    async fn authenticate(&self, ctx: &AuthContext) -> Result<BackendSession>;
    async fn list_cases(&self, q: CaseQuery) -> Result<Page<CaseSummary>>;
    async fn get_case(&self, id: &CaseId) -> Result<CaseDetail>;
    async fn save_draft(&self, case: &CaseDraft) -> Result<CaseVersionRef>;
    async fn publish(&self, v: &CaseVersionRef, req: PublishRequest) -> Result<PublishResult>;
    async fn trigger_execution(&self, case: &CaseId, plan: &RunPlan) -> Result<ExecutionRef>;
    async fn poll_execution(&self, e: &ExecutionRef) -> Result<ExecutionState>;
    async fn upload_artifact(&self, a: &ArtifactRef) -> Result<RemoteArtifactRef>;
    async fn report_result(&self, e: &ExecutionRef, r: &RunOutcome) -> Result<()>;
}
```

关键约束：

- **契约里只有领域概念**（用例、版本、执行、制品、结果），没有 URL 前缀、
  没有 token 形态、没有平台特有字段。平台差异全部关在插件内。
- `ark-studio` 只依赖 `BackendProvider` trait；`xtask lint-deps` 禁止
  `ark-studio` 依赖任何 `plugin-backend-*` 具体实现。
- 一个后端插件 = `Plugin` 的一个 `register_backend_provider` 调用，
  与工具插件走同一注册表（§7.7）。

#### 7.12.2 内置两个参照实现

| 插件 | 用途 | 说明 |
|---|---|---|
| `plugin-backend-local` | 本地文件后端 | 用例与执行全部落在会话目录 `runs/`，零外部依赖，用于离线与差分测试 |
| `plugin-backend-galaxy` | 智测方舟对接 | 现有 Galaxy 直连逻辑平移进插件，保留原字段与鉴权方式 |

`plugin-backend-galaxy` 就是"迁移旧行为"的容器：现状里的
`FACADE_PREFIX`、token 读取、`versions/{id}/compile` 等路径全部收进这个插件内部，
核心代码里不再出现 `galaxy` 字样（CI 用 `grep -ri galaxy crates/ --exclude-dir=plugin-backend-galaxy` 断言）。

#### 7.12.3 第三方后端如何接入

```bash
cargo xtask new-plugin --kind backend --id ark.example.xray --dir ../ark-plugin-xray
```

生成的插件只需实现 `BackendProvider` 与一个 `manifest.json`：

```json
{
  "id": "ark.example.xray",
  "kind": "backend",
  "requires": ["ark.builtin.studio"],
  "configSchema": { "...": "由 schemars 生成，不手写" }
}
```

两种接入形态，**都通过 ark 的基础能力与外界通信**：

| 形态 | 插件做什么 | ark 提供什么 |
|---|---|---|
| 内嵌 Rust 插件 | 实现 `BackendProvider`，用 `HttpCapability` 发请求 | 代理/TLS/重试/限流/凭据注入/审计；加入 workspace 即编译期注册 |
| 外部插件（推荐给第三方） | 实现一个 MCP server，暴露 `backend.*` 工具 | `ark-mcp` 出口 + `McpBackendProvider` 适配器；**不改 Rust 代码、不重新打包应用** |

- 外部后端插件的 MCP 工具命名统一为 `ark.example.xray.backend_list_cases` 这类
  带插件命名空间的名称，由 ark 的出口聚合，避免与其它插件冲突。
- 外部插件若要回调本应用（例如"把执行结果写回某个会话"），
  通过 ark 暴露的 MCP 工具（`turn_start` / `events_replay`）连回来，
  而不是直连数据库或私有端口。

#### 7.12.4 多后端并存与选择

- 配置里 `backend.active` 指定当前后端；每个线程可覆盖（`thread.backend_id`），
  便于"同一会话里对比两个平台"。
- 后端能力差异用**能力声明**表达：`Capability::Publish`、`Capability::ArtifactUpload` 等，
  UI 按声明隐藏不可用按钮，而不是 `if backend == "galaxy"`。
- 后端切换不迁移数据：用例 ID 是"后端命名空间 + 原始 ID"的复合键
  （`BackendRef { provider: "galaxy", id: "12345" }`），避免不同平台 ID 碰撞。

### 7.13 IM 接入插件：平台服务的样板消费者

**现状**：IM 能力散落在 Electron 主进程——微信（`claw-im-mirror-coordinator.ts`、
`claw-im-session-state.ts`、`weixin-bridge-runtime.ts`）、飞书（`feishu-streamer.ts`）、
Telegram（`telegram-runtime.ts`），每家各自实现长连接、token 刷新、会话镜像、消息路由。

**新设计**：收编为**一个业务插件** `plugin-im`（`ark.builtin.im`），
它同时是"平台服务 + 隔离约定"的验收样板。

#### 7.13.1 插件形态

每个 IM 通道是一个子 provider（微信 / 飞书 / 钉钉 / Telegram），四种方向的职责：

| 方向 | 链路 |
|---|---|
| 出站推送 | 订阅事件（`turn_completed`、`approval_requested`、定时任务结果）→ 按 `{plugin_id}.route.*` 配置匹配 → 经 `ark.http` 调通道 API |
| 入站指令 | 通道 webhook / 长连接消息 → 指令解析 → 经 MCP 工具 `ark_turn_start` / `ark_turn_steer` 驱动指定会话 |
| 界面 | `register_page` 注册"消息通道"管理页：扫码登录、通道状态、路由规则 |
| 工具 | `im_send_message` / `im_list_sessions` / `im_set_route` 注册进 MCP 目录，agent 可在会话里直接回复 IM |

#### 7.13.2 平台服务映射（逐项对号，示范"安装即得"）

| IM 插件需要 | 用哪个平台服务 | 隔离体现 |
|---|---|---|
| 调微信/飞书/钉钉 API | `ark.http` | `manifest.http.allow` 限定各通道域名；UA 自动为 `ark-plugin/ark.builtin.im@<ver>` |
| 各通道 token / app_secret | `ark.credentials` | 引用名 `ark.builtin.im/weixin_token`；代码与日志中无明文 |
| 会话路由规则 | `ark.config` | 键名 `ark.builtin.im.route.*`，schema 校验 + 热更新 |
| 会话镜像与去重状态 | `ark.kv` | 键空间固定归属 `ark.builtin.im`，配额隔离 |
| 附件/图片收发 | `ark.files` | 根 `data/plugins/ark.builtin.im/`，只给相对句柄 |
| 回合完成提醒 | `ark.notify` | 路由带 `plugin_id`，用户可单独关闭 |
| agent 回复 IM | `ark-mcp` 总线 | 工具名 `ark.builtin.im.im_send_message` |

#### 7.13.3 设计约束

- **通道实现不进内核**：新增一个 IM 通道 = 插件内加一个 provider 模块，`arkd` 不感知。
- **入站指令走同一授权链**：IM 消息驱动的 `turn_start` 与外部 MCP 客户端同级——
  必须持有 `ClientGrant`；审批决议等敏感操作仍走宿主审批流，
  **不因为"消息来自负责人"就跳过确认**。
- **凭据分通道**：一个通道的 token 泄漏不影响其它通道；轮换统一走 `ark.credentials`。
- **旧实现不迁移**：`claw-*` / `feishu-*` / `telegram-*` 只作行为参照，
  插件按新契约重写；灰度期与旧 Electron 应用并行可用。

### 7.14 对标主流 harness：能力缺口与预留接口

#### 7.14.1 对标范围与结论

参照对象：Claude Code / Codex CLI / Cursor / Cline / OpenHands / Aider 这类
"agent harness"的公共能力面。逐项与本项目（现 `kun` → 新 `ark`）对照：

| 能力面 | 主流做法 | 本项目现状 | 结论 |
|---|---|---|---|
| 工具生态 | 内置工具 + MCP + 插件市场 | 10 个内置 + MCP 客户端 | **已有**，需插件化（§7.7） |
| 技能/提示词资产 | skills 目录 + 斜杠命令 + 模板 | 有 skills，命令面弱 | 预留 `CommandProvider` |
| 上下文管理 | compaction + memory + 引用 | 已有 compaction / memory | **已有**，需下沉 Rust（§7.4） |
| 代码库理解 | 语义索引 / 符号图 / 全仓检索 | 只有 `grep`/`find`/`ls` | **缺口**：预留 `IndexProvider` |
| 权限与沙箱 | approval 模式 + OS 级沙箱 | 有 approval，沙箱偏粗 | 预留 `SandboxProfile` 平台服务 |
| 会话管理 | resume / fork / rewind / share | 有 resume/fork/rewind | **已有**；`share` 预留 |
| 扩展点 | hooks / MCP / 插件 / marketplace | hooks 有雏形 | **已有**，需纳入插件契约 |
| 多 agent 协作 | subagents / 并行任务 / teams | 有子代理（单层） | **已有**，团队编排预留 |
| IDE 集成 | LSP / diff / inline edit | 有 `lsp` 工具 | **已有**，深度集成预留 |
| 成本与预算 | token 预算 / 硬上限 / 成本面板 | 有 usage 计量 | 预留 `BudgetPolicy` |
| 评测与回归 | eval harness / 基准集 | 无 | **缺口**：预留 `EvalHarness` |
| 远端与协作 | 云端会话 / 团队共享 | 无 | 暂不做 |

#### 7.14.2 缺口清单与补齐方式

只列**当前确实没有**的能力，并说明靠什么补齐：

| 缺口 | 补齐方式 | 触发条件（何时值得做） |
|---|---|---|
| 代码库语义索引 | 新插件 `plugin-index`，注册 `IndexProvider` 端口 | 全仓检索成为高频操作，或 `grep` 命中率不足 |
| 斜杠命令 / 提示词模板 | 新插件 `plugin-commands`，注册 `CommandProvider` | 用户开始重复粘贴同一段提示词 |
| OS 级沙箱分级 | 平台服务 `ark.sandbox`（`SandboxProfile`） | 需要运行不可信代码或多租户隔离时 |
| 会话分享/导出为可复现包 | 复用 §7.4.5 的 `export`，加 `share` 元数据 | 需要把会话交给他人复现时 |
| 多 agent 团队编排 | 扩展现有 delegation（同层子代理 → 命名团队） | 单层子代理不够用时 |
| 成本硬预算 | 平台服务 `ark.budget`，基于已有 usage 计量 | 出现成本失控事故时 |
| agent 自评测基准 | `eval` 插件，注册 `EvalHarness` 端口 | 需要量化"改动是否让 agent 变差"时 |
| hook 生命周期补全 | 扩展 §7.7 插件契约（`HookPoint`） | 现有 hooks 覆盖不到的阶段出现需求时 |

#### 7.14.3 预留接口的原则（预留但不是空实现）

1. **只预留三样东西**：端口 trait、manifest 声明字段、事件主题。
   **不**预留空实现、占位模块或 TODO 分支。
2. **每个预留端口写清三件事**：最小契约、补齐时机（上表的触发条件）、
   与现有能力的关系（是替换还是新增）。
3. **补齐能力不改内核**：插件实现端口后即可用，内核不新增 `if plugin == ...` 分支。
4. **端口一旦发布即语义化版本**：破坏性变更升主版本，
   依赖它的插件按 §7.5.4 的 `requires` 规则处理。

预留端口清单（v1 只定义，不实现）：

```rust
pub trait IndexProvider: Send + Sync { /* 语义检索：index/query/invalidate */ }
pub trait CommandProvider: Send + Sync { /* 斜杠命令与提示词模板注册 */ }
pub trait BudgetPolicy: Send + Sync { /* 成本与 token 硬预算 */ }
pub trait EvalHarness: Send + Sync { /* 场景集、跑分、基线对比 */ }
pub trait SandboxProfile: Send + Sync { /* 沙箱档位与能力限制 */ }
```

这些端口与平台服务同层：实现方按普通插件注册，复用 `ark.log`/`ark.config`/`ark.http`。

#### 7.14.4 三条硬指标：快速扩展、缺陷隔离、性能

**快速扩展**（新增能力只加插件，不改内核）：

- 新增一类能力 = `cargo xtask new-plugin` 生成骨架 → 实现端口 → 注册（§7.7.4）。
- 基础设施零负担：日志/配置/HTTP/凭据/KV/文件安装即得（§7.5.4）。
- 能力自描述：插件注册后自动出现在 `GET /v1/runtime/capabilities`、
  MCP `tools/list` 与前端导航，不需要额外接入工作。

**缺陷隔离**（一个插件的问题不扩散）：

| 故障 | 隔离手段 |
|---|---|
| 插件 panic | 内置插件工具调用跑在 `catch_unwind` 边界内；外部插件在独立进程 |
| 插件超时/卡死 | 每次调用带 `CancellationToken` + 超时；超时即取消整棵子调用树 |
| 插件日志风暴 | `ark.log` 逐插件限流；日志写入失败绝不阻塞业务 |
| 插件内存/磁盘暴涨 | 平台服务按 `plugin_id` 配额（KV、files、缓存、子进程数） |
| 平台服务缺失/损坏 | 最小诊断模式启动 + 依赖它的插件 fail closed 跳过（§7.5.4） |
| 插件互相调用打环 | 调用链深度上限 + 循环检测（§7.5.2） |
| 门禁/证据造假 | `Verified<T>` 只能由验证器构造（§7.8.4） |

**性能**（关键路径不能因为插件化而变慢）：

- **进程内插件零序列化**：`Arc<dyn Trait>` 直接调用，不经 JSON、不经 IPC。
- **MCP 互调不落盘不经网**：进程内路由，只有外部插件才走协议。
- **启动时算一次**：工具 schema canonical 排序 + catalog fingerprint + 前缀校验在启动完成。
- **热路径无锁**：每会话单 writer actor；共享状态用 `arc_swap` 读多写少；
  避免全局 `Mutex` 与跨会话争用。
- **连接与缓存复用**：平台服务持有共享 `reqwest::Client`（keep-alive）与统一 TTL 缓存，
  插件各自建连接池是禁止项。
- **性能预算进 CI**：单轮往返延迟、工具派发开销、插件调用 p99、
  5000 事件会话重放首包，均设阈值门禁（§10.2）。

### 7.15 配置热更新与"最小重启"

**目标：除少数底层项外，所有配置变更都不重启。**
只有改变**进程级资源或安全边界**的配置才允许要求重启；其余一律热更新。

#### 7.15.1 配置分级（写在类型上，不是文档约定）

```rust
pub enum ReloadClass {
    Hot,           // 热生效：不重启、不断连
    HotRelisten,   // 热生效但需重建监听（TLS 轮换）——SSE 短暂闪断后按 since_seq 重连
    Restart,       // 必须重启 arkd 进程
}
```

每个配置字段在 Rust 类型上用 `#[config(reload = Hot)]` 标注，`schemars` 生成 schema 时
**带上该分级**；`GET /v1/runtime/config/schema` 暴露给 GUI，
用于渲染"改这一项需要重启"的提示。分级是类型系统的一部分，
新增字段必须显式选择，默认不写就是编译不过。

#### 7.15.2 分级清单

| 类别 | 典型字段 | 生效方式 |
|---|---|---|
| **热更新（绝大多数）** | 模型 provider 表、API key / baseUrl / endpointFormat、fallback 链、子代理预算、loop 调参（预算 / 风暴阈值 / 参数修复）、连接器环境变量、审批策略、沙箱档位、MCP server 列表、memory / 附件开关、技能目录、日志级别、**插件启用状态与插件配置**、通知偏好、主题 | `POST /v1/runtime/config` → 原子替换快照 |
| **热更新 + 重建监听** | TLS 证书 / 私钥轮换 | 新证书立即生效；现有 SSE 闪断一次，客户端按 `since_seq` 重连 |
| **必须重启** | 监听端口、绑定地址（loopback / 远程）、数据目录、runtime token 与安全边界开关、日志落盘路径、进程级沙箱模式、平台服务静态装配变更 | 明确拒绝并返回字段列表 |

判定标准一句话：**改的是"进程握着的资源/安全根基" → 重启；只是换客户端、换策略、换开关 → 热更新。**

#### 7.15.3 机制

```rust
pub struct ConfigStore {
    snapshot: ArcSwap<ConfigSnapshot>,   // 读侧无锁；读者永远看到自洽快照
    version: AtomicU64,
}

pub enum ApplyResult {
    Applied  { version: u64, changed: Vec<FieldPath> },
    Rejected { restart_required: Vec<FieldPath>, reason: String },
}
```

- **单一入口**：`POST /v1/runtime/config` + `ConfigPatch`（取代 §2.6 的四条独立路由）。
- **事务性**：先全量校验（schema + 跨字段约束 + 被依赖能力是否可用）→ 通过才替换快照；
  失败保持旧快照并返回被拒字段与原因，**不存在"改了一半"**。
- **订阅者显式有序**：模型客户端池 → 工具注册表 → 插件运行时 → 限流/配额器，
  各自实现 `on_config_change(&delta)`，调用顺序在代码里写死并加测试。
- **重启项整批拒绝**：patch 中含 `Restart` 字段时整批拒绝并返回
  `restart_required` 列表；GUI 据此提示"需重启，涉及：port"。
  **不静默忽略，也不做部分应用。**
- **可观测**：每次应用落一条 `config_applied` 事件（版本号 + 变更字段 + 发起者），
  可回溯"谁在什么时候改了哪一项"。

#### 7.15.4 让"必须重启"的代价最小

1. **只重启 `arkd` 子进程，不重启桌面外壳**：Tauri 主进程存活，
   导航、主题、当前页面不丢。
2. **会话不丢**：会话目录即系统记录（§7.4），重启后重新加载；
   SSE 客户端按 `since_seq` 自动重连，事件零丢失。
3. **在途回合有明确策略**：重启前进入 draining，对已开始的回合二选一——
   "等待落盘（带上限）"或"按 `turn_aborted` 明确中止"，由配置决定；
   **不允许半途丢弃造成事件流断裂**。
4. **重启有确认与回执**：GUI 提示"需重启以应用 port 变更"，
   确认后执行，重启完成回到原页面并提示"已应用"。
5. **把重启项压到最少**：新配置**默认按 `Hot` 设计**，
   只有证明无法热更新才降级为 `Restart`；新增 `Restart` 字段需评审理由并写入变更说明。

#### 7.15.5 与插件的关系

- 插件**配置**热更新：`ark.config` 按 `{plugin_id}.{key}` 分区更新，
  只通知该插件，**不重启进程，也不影响其它插件**（§7.5.5）。
- 插件**启用/停用**热生效：注册表支持运行期注册/反注册（§7.6），
  停用即下线其工具与页面。
- 插件**代码升级**在 v1 静态装配下需要重启——这是唯一例外，已在 §7.5.4 说明；
  未来做动态分发时再单独评估。

#### 7.15.6 验收

- **热字段逐个验证**：改动后进程 PID 不变、无重启、1s 内生效（含插件配置）。
- **重启字段验证**：patch 被整批拒绝并返回准确字段列表；GUI 显示重启提示。
- **重启连续性**：`kill arkd` → 重启 → SSE 按 `since_seq` 重连，事件零丢失、会话可继续
  （与 §10.2 崩溃恢复共用用例）。
- **事务性**：构造含非法值的 patch，断言旧配置**完全未被修改**。
- **隔离性**：改 A 插件配置，断言 B 插件与内核不受影响、无重启。

### 7.16 与现成 harness 的差距（DSH / omp 对标结论）

完整证据见 `docs/harness-gap-analysis.md`。结论摘要：

**没有任何现成 harness 等于我们的目标架构。**

| 参照物 | 语言 | 与我们的关系 |
|---|---|---|
| **deepseek-harness (DSH)** | TS / Node≥22，MIT，256k★ | 口号就是 "Everything is a Plugin"，插件内核是 vendored **cordis**；概念最接近，**但缺隔离层、缺平台服务分层、缺最小重启、缺 MCP 双向** |
| **oh-my-pi (omp，即 pi harness)** | TS(Bun) + Rust(napi)，26k★ | "通用内核 + 产品层 + Rust 热路径"；插件化程度高但**不构成能力总线** |
| **cordis**（DSH 的插件内核） | TypeScript，9.3k★ | 插件 + 服务 DI + **可撤销副作用**；有论文（arXiv 2608.25512） |
| **cordis-rs**（`eavae/`、`y0usaf/`） | **Rust** | cordis 的 Rust 实现（含 revertible effects + Lua）——**我们最直接可借鉴的实现范式** |
| **deepseek-harness-desktop** | Tauri | 安装包仅 **8MB**，验证了 §7.10.1 的 Tauri 选型与体积判断 |
| **dsh-plugin / awesome-deepseek-harness** | — | DSH 插件生态的运营样本，可参考插件发布形态 |

**我们明显领先、应保持的 6 点**：多租户隔离默认强制（DSH/omp 插件共享进程全局）、
配置分级与最小重启、MCP 双向单一出口 + 插件互调总线、会话目录即迁移单元、
后端可插拔、单 agent 的可审计性（DSH 允许模型写 JS 编排脚本 + 运行时自修改，灵活但难审计）。

**必须补的 8 项**（都在既有端口体系内补，不改内核）：

| # | 缺口 | 借鉴来源 | 落点 |
|---|---|---|---|
| 1 | **可撤销副作用**：卸载插件要能回滚注册项之外的副作用（连接、写入、监听） | cordis revertible effects / cordis-rs | §7.7.1：`Registration` → **`EffectScope`**（RAII + revert） |
| 2 | **waterfall 事件**：可拦截/改写/短路的事件管道 | DSH 三类事件 + waterfall | §7.14.3：预留 `HookPoint` 升级为 **`Interceptor`**，改写需 grant |
| 3 | **显式运行时不变量集**：把隐性约定写成启动即校验的断言 | DSH 40+ invariants、`Model-visible ⟺ logged` | §7.16.1（见下） |
| 4 | **提示词分段与覆盖**：`PromptSection` 组合，但强制校验前缀稳定 | DSH PromptSection order + waterfall | §7.14.3 预留，与 §7.4 前缀校验联动 |
| 5 | **真实 tokenizer 端口**：默认启发式，可插 tiktoken-rs | omp 原生 BPE | §7.14.3 预留，同时收敛 §2.2 B-2 的三份口径 |
| 6 | **DAP 调试能力** | omp DAP 28 ops | §7.14.2 的 `CodeIntelligence` 端口补 DAP |
| 7 | **凭据轮换与 fallback 联动** | omp credentials rotation retry | §7.5.4 `ark.credentials` + §7.2 fallback 链 |
| 8 | **插件生态运营**：清单/审核/分发 | DSH `dsh-plugin` topic + awesome 列表 | §7.7.3 catalog 与 §12.2 市场待决项 |

#### 7.16.1 运行时不变量（新增，进 CI）

借鉴 DSH 的"40+ 运行时不变量"，把它们写成**启动即校验、失败即拒绝启动**的断言，
而不是散落的注释。首期至少覆盖：

| 不变量 | 含义 |
|---|---|
| `Model-visible ⟺ logged` | 发给模型的历史必须全部落过盘；反之落盘的内容不必都发给模型 |
| 前缀稳定 | 同一会话内 system prefix 的 fingerprint 不变（§7.4 已有实现，此处升级为硬断言） |
| seq 单调无洞 | 每线程事件 `seq` 严格递增，压缩产生的空洞必须显式标记 |
| 工具调用配对 | 每个 `tool_call` 必有 `tool_result`，孤儿在发送前被修复 |
| 插件身份唯一 | `plugin_id` 在注册表内唯一，且与调用上下文一致 |
| 凭据不出插件 | 插件代码路径上不存在凭据明文（含日志与错误） |

这些断言在启动自检与每轮回合的调试模式下运行；CI 用固定场景验证断言会被触发。

## 8. 问题 → 设计对策对照表

> 这张表是本文的核心。每条现状问题都必须在此有对应的结构性对策，而不是"注意一下"。

| 现状问题 | 新架构对策 | 验证方式 |
|---|---|---|
| C-3 错误码多份手抄 | Rust 单一真源 + `ts-rs`/`schemars` 生成；生成物 diff 即 CI 失败 | `cargo xtask gen --check` |
| C-1 脱敏逻辑跨包双活副本 | 脱敏只在 `ark-common::redact` 实现；前端经 Tauri command 调用，不保留 TS 副本 | 双端对同一测试向量输出一致（§6.1） |
| C-2 路径归一化已漂移 | 归一化只由 `ark-common::path` 实现，**前端经 WASM（R2）调用同一份代码**，不保留 TS 实现 | 共享测试向量 + WASM 与原生构建对同一输入的输出一致性测试 |
| C-4 用量字段静默丢失 | 全链路唯一 `Usage`，禁止投影结构 | 类型层面无法构造子集 |
| C-5 枚举不一致 | 枚举由 Rust 生成 TS | 生成物 diff |
| C-6 代理实现两份 | 唯一 `reqwest::Client` 工厂注入 | 依赖图检查：`ark-llm` 之外无 HTTP 客户端 |
| B-1 canonicalize 7 份 | 唯一 `ark_contract::stable_json` | `grep` 断言全仓仅一处实现 |
| B-2 token 估算 3 份 | 唯一 `TokenEstimator` trait + 一个默认实现 | 压缩与裁剪共用同一实例 |
| B-3 UTF-8 截断切断字符 | 唯一 `truncate_utf8`，基于 `char_indices` | property test：任意输入输出均为合法 UTF-8 |
| B-5 渲染层重复纯函数 | 映射下沉 Rust；前端只剩分组排序 | 单文件行数 + 重复检测 CI |
| 2.3 巨型单体 | 状态机拆分 + crate 边界 + 行数上限 | clippy 自定义 lint / `tokei` 门禁 |
| 2.4 死代码 2200 行 / 1030 导出 | 默认 `pub(crate)`；`cargo-udeps` + `cargo-machete` | CI 报告 |
| 2.5 SSE 重放竞态 | 单 writer actor 分配 seq；`subscribe` 在同一临界区完成"定水位→读积压→注册实时" | 重放压力测试：100 次重连零丢零重；并发追加下 subscribe 无空窗 |
| 2.6 四条热更新路由各自实现 | 统一 `ConfigPatch` + 版本号 + 回滚 | 每条 patch 的单测 + 并发 patch 集成测 |
| 2.6 `process.env` 全局污染 | `ToolContext.env` 快照 | 多连接器并发测试 |
| S-1 spawn 无 error 监听致崩溃 | 统一 `RunnerProcess` 构造器，返回 `Result` | 故意 spawn 不存在路径，断言不 panic |
| S-2 队列被 reject 毒化 | 串行 actor + 单条失败隔离 | 注入写失败后继续写入仍成功 |
| S-3 假 typecheck 门禁 | `Verified<T>` 只能由验证器构造 | 编译期无法伪造 |
| S-4 制品无界增长 | `TempDir` 所有权 + 显式 `persist` + 保留策略 | 跑 100 次后磁盘占用有界 |
| S-5 变量快照明文 | 默认不落盘；落盘加密 | 文件内容断言不含明文口令 |
| S-6 门禁可外部置真 | 门禁仅由状态机产出 | 同上 S-3 |
| 子/父会话分处两个平级目录，迁移必须搬两处 | 会话目录自包含：`threads/<id>/children/<childId>/`，相对路径入盘 | 迁移测试：拷贝单目录到新机器，父子会话均可读可继续 |
| 删除父会话依赖补偿逻辑清理子会话 | 删目录即级联删除；有活动子任务时先用 `CancellationToken` 取消并显式确认 | 杀进程/删目录后无孤儿目录残留 |
| 目录型存储缺少跨会话搜索 | 可重建索引（`index.sqlite`）只存元数据，不存正文；真源仍在目录 | 删除索引后重启自动重建，搜索结果一致 |
| MCP 只有客户端方向，agent 无法操作本应用 | 新增 MCP server（§7.6.2），把线程/回合/审批/用量/Studio 暴露为工具 | MCP Inspector 逐工具调用通过；与 HTTP 路径产生同一事件 |
| 扩展点散落在 `runtime-factory.ts` 硬编码 | 插件注册表 + manifest + 两类插件（内置/MCP）同一契约 | 新增一个插件不改任何既有 crate |
| 模块边界靠注释维持，重复实现跨包滋生 | 对齐 QDC：按域切 crate + 模块内固定分层 + `pub use` 白名单 | `xtask lint-deps` 依赖方向 + `xtask lint-layers` 零违规 |
| **S-7 UI 自动化内嵌第二个 agent 循环** | **只保留 `ark` 一个 agent**：UI 自动化降级为工具 + 步骤执行器 + 用例模型，删除 Playwright Test Agent 的决策层（§7.8.0） | 全局断言只有一处 `run_turn` 循环；UI 自动化场景的模型调用次数与轮次可归因到 `ark` 的 turn |
| **S-8 后端写死智测方舟** | `BackendProvider` 端口 + `plugin-backend-*` 插件；核心 crate 零平台字样（§7.12） | `grep -ri galaxy crates/ --exclude-dir=plugin-backend-galaxy` 无命中；换后端只加插件 |
| 插件各自造通信层（代理/重试/凭据各写一遍） | **ark 提供 `ark-http` 基础能力**，插件只声明 allowlist 与凭据名（§7.5.1） | 依赖图：插件 crate 不得依赖 `reqwest`/`hyper` 客户端 |
| 每个插件各起 MCP server、开端口 | **ark 提供唯一 MCP 出口**，插件 `expose` 注册工具；互调走 `call` 总线（§7.5.2） | 依赖图：插件 crate 不得依赖 `axum`/MCP server 端；全进程只监听一个 MCP 端点 |
| 界面硬编码，第三方页面无法接入 | **`ark-ui` 界面总线**：Code 模式本身是 `plugin-code-ui`，其它插件 `register_page` 挂页面（§7.5.3） | 停用 `plugin-code-ui` 后应用仍启动，仅剩导航骨架 |
| 基础设施每个插件各写一遍（日志/配置/凭据/KV/文件） | **平台服务插件**：`ark.log` / `ark.config` / `ark.credentials` / `ark.kv` / `ark.files` 官方发布，安装即得（§7.5.4） | 插件 crate 内不得出现自建日志/配置实现；服务缺失时依赖插件 fail closed |
| 多插件共用无隔离，日志/配置互相污染 | **默认强制命名空间**：日志带 `plugin=<id>`、配置键 `{plugin_id}.{key}`、存储/凭据/缓存/工具名/MCP 工具全部按 `plugin_id` 分区（§7.5.5） | 交叉读写测试：A 插件读不到 B 的配置/KV/凭据 |
| IM 能力散落主进程，每家各实现一遍 | 收编为 `plugin-im` 业务插件，消费平台服务（§7.13） | IM 插件只经 `ark.http`/`ark.credentials`/`ark.kv` 通信，且凭据日志无明文 |
| 对标主流 harness 存在能力缺口（语义索引/评测/预算等） | **只预留端口**（`IndexProvider` / `CommandProvider` / `BudgetPolicy` / `EvalHarness` / `SandboxProfile`），补齐时只加插件（§7.14） | 预留端口清单与触发条件成文；补齐不改内核 |
| 配置无生效分级，改端口与改日志级别同样要重启 | **配置分级写在类型上**（`Hot` / `HotRelisten` / `Restart`）；热项经单一 `ConfigPatch` 原子生效，重启项整批拒绝并提示（§7.15） | 逐字段测试：热项改后 PID 不变且 1s 内生效；重启项准确返回字段列表 |
| 多会话事件与审计可能形成全局瓶颈 | 事件路由按线程/插件分片；安全审计独立单 writer 且 fail closed；订阅者全部有界隔离（§7.4.6） | 过载压测下线程 writer 不被慢订阅者阻塞；安全审计零丢失 |
| 各线程 writer 并发更新 SQLite，索引水位与投影可能错位 | 唯一 `IndexProjector` 批量事务更新元数据/FTS/`indexed_seq`；满队列转 dirty set，启动对账兜底（§7.4.2） | 注入事务失败、队列满、kill -9 后索引最终与事件日志一致 |

---

## 9. 迁移策略

核心原则：**双运行时并行，差分证明等价，再切流**；
并行期任何一个数据目录**同一时刻只有一个写者**。

### 9.0 写权限规则（先定规则，再谈并行）

| 阶段 | 数据目录所有者 | 另一侧权限 |
|---|---|---|
| M0–M1 | 旧运行时（Node） | Rust 只读历史数据，或读导出副本 |
| M2–M6 | 旧运行时写主目录；Rust 写**独立测试目录** | 两侧目录物理隔离，不共享 `data/` |
| M7–M8 | 由配置 `writer_owner` 指定，切换需停进程 | 非所有者进程拒绝启动写模式 |
| M9 灰度 | 默认旧运行时；用户显式切到 Rust 后，旧进程先停 | 切换 = 停旧 → 迁移/校验 → 起新 → 写标记文件 |
| M9+ | Rust | 旧运行时降级为只读导出工具 |

强制手段：

- `data/.writer-owner` 文件记录当前所有者与 PID；启动时校验，不匹配则拒绝写模式。
- 文件锁（§7.4.3）之外再加这层逻辑锁，避免"两个不同二进制各自以为自己是唯一"。
- 回退：`arkd export --all` 产出快照，旧运行时从快照恢复后重新取得所有权；
  不允许"直接切回去继续写同一个目录"。

### 9.1 差分测试台（先建，再写代码）

```
xtask differential
  ├─ 场景脚本（YAML）：创建线程 → 发消息 → 触发工具 → 审批 → 中断 → 恢复
  ├─ 对旧运行时录制：HTTP 交互 + SSE 事件流 + 最终存储状态
  ├─ 对新运行时回放：mock provider（录制响应）+ 断言事件流
  └─ 归一化后逐字节 diff（忽略 timestamp、uuid、duration）
```

归一化规则显式列出：时间戳、ID、耗时、随机顺序的并行工具结果
（后者应按 call 顺序归一化，与现状语义一致）。

### 9.2 里程碑

| 阶段 | 交付 | 退出标准 |
|---|---|---|
| M0 | 契约冻结：从现有 TS 提取全部类型 → `ark-contract`；生成 TS 绑定 | 生成物能被现有渲染层编译通过（不改渲染层逻辑） |
| M1 | `ark-store`（会话目录布局 + 索引）+ `ark-server` 骨架；只读接口（threads/items/events 重放） | 指向历史数据目录，GUI 能正确显示历史会话；`import-jsonl` 按 §7.4.5 顺序（父 → 子正文 → `runs.jsonl` → 重建索引 → 才 orphan cleanup）迁移后数据无损，且记录缺失/重复必须报非零退出码 |
| M1.5 | 会话迁移闭环：`export` / `import` 子命令 + 相对路径校验 | 拷贝单会话目录到另一路径，父子会话均可打开并继续对话 |
| M2 | `ark-llm` + 最小 `ark-agent`（纯文本回合，无工具） | 差分：单轮 / 多轮文本对话事件流一致 |
| M3 | 工具系统（read/bash/edit/write/grep/find/ls/git_inspect/run_script/lsp） | 差分：含工具调用的完整会话一致 |
| M4 | 审批、用户输入、steering、中断、compaction、fork、rewind | 差分：闸门交互场景一致 |
| M5 | 子代理 delegation + 子会话事件重放 | 差分：子代理场景一致 |
| M6 | MCP 客户端收敛、附件、memory、skills、usage 诊断、热更新 | 全部诊断接口字段级一致 |
| M6.5 | **能力总线**：`ark-http` + `ark-mcp` 出口/互调 + `ark-ui` 页面注册；MCP 服务端（线程/回合/审批/用量/Studio 工具）；插件注册表与脚手架 | 三个参照插件（`plugin-code-ui`、`plugin-backend-local`、`plugin-studio`）全部只经总线通信；`cargo xtask new-plugin` 生成的插件可加载并出现在能力目录 |
| M6.6 | **平台服务 + 隔离**：`ark.log` / `ark.config` / `ark.credentials` / `ark.kv` / `ark.files`；`PluginIdentity` 与 grant；命名空间强制 | 交叉读写测试全绿（插件间配置/KV/凭据互不可见）；停用任一平台服务时依赖方 fail closed 且主进程可启动 |
| M6.7 | **IM 插件** `plugin-im`（微信/飞书/钉钉/Telegram 至少两个通道） | "IM 消息 → 回合 → 审批 → 回复"全链路跑通；凭据只存在 `ark.credentials`，日志无明文 |
| M6.8 | **配置分级与最小重启**：`ReloadClass` 标注 + `ConfigStore` 快照 + `ConfigPatch` 单一入口 + draining 重启 | 热字段改后不重启即生效；端口变更整批拒绝并提示；重启后 SSE 按 `since_seq` 重连零丢事件 |
| M7 | `ark-studio` 插件化编排层 + sidecar 协议；**删除 Playwright Test Agent 第二层循环** | 用现有用例跑通，`runner-event/v1` 字段兼容；UI 自动化全流程只有 `ark` 一个决策循环（§7.8.0 断言） |
| M7.6 | `ark-ports::BackendProvider` 端口 + `plugin-backend-local` / `plugin-backend-galaxy`；后端选择与能力声明 | 切换后端不改核心代码；`grep galaxy` 仅命中插件目录 |
| M8 | Code 模式渲染层重写 + Tauri 外壳 | 交互走查清单全绿 |
| M9 | 灰度：默认旧运行时，新运行时可选；一个月后翻转 | 崩溃率、p95 延迟、缓存命中率均不劣于基线 |

### 9.3 回退策略

- 两个二进制并存：**`ark`（Node，原 `kun`）** 与 **`arkd`（Rust）**，端口不同。
- Tauri 侧通过配置切换目标运行时；切换不需要重新打包。
- 数据目录**双向可读**：Rust 侧实现 `import-jsonl`，也保留 `export-jsonl`。
- **"JSONL 兼容层"的准确定义**：它指的是**磁盘文件格式**（`events.jsonl` / `items.jsonl`），
  不是"额外的第二条写入路径"。灰度期每个目录仍按 §9.0 只有一个 owner 写入；
  Rust 接管后写的也是同一套 JSONL 格式，因此旧运行时随时能读。
  任何时候都不存在"两个进程同时写同一目录"的配置。

---

## 10. 验收标准

### 10.1 功能等价

- §1.3 清单逐条勾选，每条有集成测试
- 差分测试台 30+ 场景全部 diff 为 0
- 会话迁移：随机抽 20 个历史会话，导出后在新数据目录导入，事件流与 item 流逐字节一致（归一化 ID/时间戳）
- MCP：`tools/list` 与 `tools/call` 覆盖全部暴露工具；一次经由 MCP 的审批决议产生的 SSE 事件与 HTTP 路径完全一致
- 插件：删除任意外部插件后进程正常启动；新增插件不触发既有 crate 变更（`git diff --name-only` 校验）
- 单 agent：静态检查全仓只有一处回合循环入口；UI 自动化运行期间模型调用全部归属 `ark` 的 turn
- 后端插件：用 `plugin-backend-local` 与 `plugin-backend-galaxy` 跑同一套用例，编排层零改动
- 写权限：并行期故意让两个运行时同时指向同一 `data/`，断言第二个必须拒绝启动写模式
- C-2 一致性：`normalize_path` 的原生与 WASM 构建对同一组向量输出逐字一致
- MCP 授权：未授权客户端只可见公开工具；敏感操作必须产生 `approval_requested` 并由宿主决议，传 `confirm: true` 不生效
- 单 agent 静态约束：`ark-studio` / `plugin-studio` 依赖里不含 `ark-llm`，且无独立 `tokio::spawn` 决策循环
- 能力总线（HTTP）：插件 crate 的依赖里不含 `reqwest`/`hyper` 客户端；越出 `manifest.http.allow` 的请求被拒绝并落 `http_denied`
- 能力总线（MCP）：全进程只暴露一个 MCP 端点；插件 A 经 `mcp.call` 调用插件 B 的工具成功，且调用链 `trace_id` 贯通
- 能力总线（UI）：停用 `plugin-code-ui` 后应用正常启动；新增一个插件页面只需 `register_page`，不改外壳路由
- 插件互调边界：未在 `mcp.consume` 声明的跨插件调用被拒绝；调用深度超限（>3）返回显式错误而非栈溢出
- 隔离强制：插件 A 无法读/写插件 B 的配置、KV、凭据；每条日志含 `plugin=<plugin_id>`；配置落盘键形如 `{plugin_id}.{key}`
- 平台服务：插件 crate 内无自建日志/配置/凭据实现；停用 `ark.kv` 后依赖它的插件 fail closed，其余插件正常
- 预留接口：`IndexProvider` 等 5 个端口只定义不实现；新增一个端口实现插件不改内核（`git diff` 仅在插件目录）
- IM 插件：至少两个通道跑通全链路；`grep -r` 插件代码与日志无 token 明文；入站指令的审批仍走宿主审批流
- 配置热更新：全部 `Hot` 字段逐个验证"改完即生效、进程 PID 不变"；插件配置变更不影响其它插件与内核
- 最小重启：`Restart` 字段整批拒绝并返回准确清单；重启仅换 arkd 子进程，外壳与会话不丢，SSE 重连零丢事件
- 配置事务性：含非法值的 patch 被拒后，旧配置逐字段比对无变化
- 子会话迁移：缺记录 / 重复 `child_id` / 父不存在三种场景均报显式错误且不留半个迁移现场
- 索引单写者：除 `IndexProjector` 外任何模块持有 SQLite 写连接均由静态检查失败；事务故障后 `indexed_seq` 不超前
- 全局事件/审计：慢订阅者与日志风暴不阻塞线程 writer；安全敏感操作在审计不可用时 fail closed

### 10.2 非功能

| 指标 | 目标 | 测量方式 |
|---|---|---|
| prompt cache 命中率 | ≥ 现状基线（热态 > 90%） | 真实 provider 原生 usage 字段 |
| 冷启动到 `/health` 可服务 | < 300 ms | release 构建、冷/热各 30 次，报告 p50/p95 |
| 空闲内存（RSS） | < 现状 Electron 主进程 + kun 之和 | 同功能插件集，稳定 10 分钟后采样 |
| agent 单轮编排开销（排除模型/工具 I/O） | p99 < 5 ms，且不劣于旧运行时 | 固定 mock provider/tool，10k 轮基准 |
| 进程内工具派发（schema 已编译，排除工具执行） | p99 < 100 μs（并发 256） | Criterion，1 KiB 参数 |
| 进程内插件总线调用（含授权与 schema 校验） | p99 < 1 ms（并发 256） | Criterion，A→B，1 KiB 参数 |
| 全局事件路由 | 持续 20k events/s 5 分钟；p99 < 5 ms；无界内存增长为 0 | 1000 线程 + 32 插件混合压测 |
| 安全审计 | 压测与 kill -9 下已确认操作零丢失；fsync ack p99 < 20 ms | 独立审计压力/故障注入测试 |
| 索引投影 | 持续 5k delta/s；队列满不阻塞 thread writer；最终水位一致 | SQLite 批处理 + queue-full + kill -9 测试 |
| 5000 条事件线程的重放首包 | < 100 ms | 基准测试 |
| SSE 慢客户端 | 有界内存，不 OOM；不阻塞 writer；断开后可补齐 | 限速客户端压测 |
| 崩溃恢复 | 任意时刻 kill -9，重启后会话可继续 | 随机杀进程 100 次 |

### 10.3 代码质量门禁

- `cargo clippy -- -D warnings`
- `cargo deny check`（依赖图方向、许可证、重复依赖）
- `cargo udeps` 零未使用依赖
- 单文件 ≤ 800 行、单函数 ≤ 80 行（`xtask lint-size`）
- 分层合规：`xtask lint-layers` 校验每个 crate 只允许出现 `api/service/domain/store/config/exec` 目录，且跨层引用方向单一
- 免费午餐检查：`pub use` 白名单外的新增导出直接失败（对齐 §5.1 硬约束）
- 覆盖率：`ark-contract`、`ark-agent`、`ark-store` ≥ 80%
- 生成物 diff 为空

---

## 11. 主要风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| 行为等价难以完全证明 | 灰度期出现长尾 bug | 差分测试台 + 双运行时 + 一键回退 |
| 模型流式解析细节多（三种 endpoint 形态） | 特定 provider 下解析错 | 用录制真实响应做 fixture，逐帧断言 |
| Playwright sidecar 保持 Node 依赖 | "纯 Rust" 目标打折 | 明确边界：只有浏览器驱动是 Node，其余全 Rust；保留 `BrowserDriver` 替换点 |
| Tauri 迁移引入新的权限模型问题 | 文件访问、shell 权限 | 显式 capability 清单，最小授权 |
| 迁移期双写导致数据不一致 | 用户会话损坏 | 按 §9.0 的写权限规则：同一目录同一时刻只有一个写者，切换必须停进程 + 校验 + 写 owner 标记 |
| 团队 Rust 熟练度 | 交付速度 | 先做 `ark-contract` + `ark-store` 这类低风险高收益部分建立模式 |
| 删除 Playwright Test Agent 后生成质量下降 | UI 自动化用例质量回退 | 先用差分对比"双 agent 产物 vs 单 agent 产物"，把 Test Agent 的有效提示词与工具集平移到 `ark` 的工具描述里；保留回退开关到 M9 |

---

## 12. 待决问题（需要产品/技术负责人拍板）

### 12.1 已按最新要求定案（不再作为开放问题）

| 议题 | 结论 | 依据 |
|---|---|---|
| 后端命名 | `kun` → **`ark`**；Node 侧二进制名为 `ark`（原 `kun serve`），Rust 侧集合名 `ark-rs` | 用户要求 |
| agent 数量 | **只有 `ark` 一个决策循环**；Playwright Test Agent 的第二层循环删除 | 用户要求 + §2.7 S-7 证据 |
| UI 自动化形态 | **实现为 `ark` 的一个插件**（`StudioPlugin`），可单独启用/停用 | 用户要求 + §7.8.1 |
| 后端集成 | **`BackendProvider` 端口 + 插件**，核心零平台硬编码 | 用户要求 + §2.7 S-8 |
| 会话目录 | 子 agent 内联父目录，**v1 深度上限 = 1** | 用户要求 + 现状执行语义 |
| ark 的定位 | **能力总线**：统一提供 HTTP 出站、MCP 开放/互调、界面承载三组基础能力 | 用户要求（插件复用 ark 基础能力） |
| 插件通信 | 插件**不得**自建 HTTP 客户端 / MCP server / 界面容器，只能调用 `PluginContext` 的三组能力 | 同上，避免重演 §2.1 C-6 的重复实现 |
| Code 模式界面 | 本身是 `plugin-code-ui`，与第三方页面同等地位 | 用户要求（界面也走插件） |
| 插件互调 | 经 `mcp.call(provider, tool, args)` 进程内路由，带深度限制与循环检测 | 用户要求（插件互相调用彼此注册的能力） |
| 基础设施发布形态 | **平台服务插件**（三层模型）：内核端口不可换，官方服务实现可独立版本化 | 用户要求（日志/HTTP/配置等做成插件供其它插件使用） |
| 多插件隔离 | **默认强制**：`plugin_id` 宿主注入不可自报；日志带插件名、配置 `{plugin_id}.{key}`、存储/凭据/缓存/工具名全分区；manifest 无开关可关 | 用户要求（默认多个插件会使用） |
| IM 接入 | 收编为 `plugin-im` 业务插件，作为平台服务与隔离约定的样板 | 用户要求（补充 IM 插件支持） |
| 能力缺口补齐方式 | 只预留端口 + 触发条件；补齐时只加插件，不改内核 | 用户要求（快速扩展、预留接口） |
| 配置生效策略 | **最小重启**：仅端口/绑定地址/数据目录/token 等底层项需重启；其余全部热更新；分级写在类型上 | 用户要求（除底层配置外都热更新） |
| 存储真源 | 会话目录（JSONL）+ 可重建索引（SQLite 仅派生） | 用户要求（可迁移）推导 |

### 12.2 仍需拍板

1. **界面框架**：Tauri 2（推荐）还是保留 Electron、只换后端？
   影响：安装包体积、主进程能力迁移成本、更新机制。
2. **Playwright 长期归属**：接受 Node sidecar 常驻，还是投入 `chromiumoxide` 自研？
   影响：是否能做到单二进制分发。
3. **数据格式切换时机**：灰度期是否必须保持 JSONL 可回读？
   影响：M1 的存储实现复杂度。
4. **Write / Connect / 工作流**：本次确认排除，但它们是同一进程内的功能。
   剥离后是"保留在旧 Electron 应用"，还是"新应用不提供"？
   需在 **M0 出口**前给出结论，否则 M8 的 Tauri 外壳无法定版。
5. **许可证**：当前 `kun` 是 `PolyForm-Noncommercial-1.0.0`。
   需在 **M0 前**核查：代码、协议、图标/UI 资产、生成物、第三方依赖分别适用何种许可，
   尤其确认 Rust 重写 + 二进制分发是否会改变既有使用边界。
6. **更名影响范围**：`kun` 出现在用户可见文案、设置文件键名
   （`agents.kun.*`）、数据目录（`~/.kun/data`）、日志字段中。
   建议方案：**读旧写新**（`ark-config::legacy` 单点识别旧键/旧目录，
   首次启动时提示一次性迁移），既不改旧数据也不长期双写。
   需确认：迁移是自动执行还是用户显式确认；旧目录保留多久。

---

## 附录 A：当前 HTTP 路由全集（从 `kun/src/server/routes/index.ts` 实测）

```
GET    /health
GET    /v1/runtime/info
GET    /v1/runtime/tools
GET    /v1/skills
POST   /v1/attachments
GET    /v1/attachments/diagnostics
GET    /v1/attachments/:id
GET    /v1/attachments/:id/content
GET    /v1/memory
POST   /v1/memory
GET    /v1/memory/diagnostics
PATCH  /v1/memory/:id
DELETE /v1/memory/:id
GET    /v1/workspace/status
GET    /v1/threads
POST   /v1/threads
GET    /v1/threads/:id
PATCH  /v1/threads/:id
DELETE /v1/threads/:id
POST   /v1/threads/:id/fork
GET    /v1/threads/:id/skills
POST   /v1/threads/:id/skills/refresh
GET    /v1/threads/:id/goal
POST   /v1/threads/:id/goal
DELETE /v1/threads/:id/goal
GET    /v1/threads/:id/todos
POST   /v1/threads/:id/todos
DELETE /v1/threads/:id/todos
POST   /v1/threads/:id/turns
POST   /v1/threads/:id/rewind
POST   /v1/threads/:id/review
GET    /v1/threads/:id/turns/:turnId
POST   /v1/threads/:id/turns/:turnId/steer
POST   /v1/threads/:id/turns/:turnId/interrupt
POST   /v1/threads/:id/compact
GET    /v1/threads/:id/events            (SSE)
POST   /v1/approvals/:id
POST   /v1/user-inputs/:id
POST   /v1/user-input/:id                (兼容别名)
POST   /v1/sessions/:id/resume-thread
POST   /v1/shutdown
POST   /v1/runtime/model-providers
POST   /v1/subagents/config
POST   /v1/runtime/tuning
POST   /v1/connector-env
GET    /v1/usage
GET    /v1/debug/llm-rounds
GET    /v1/child-runs
GET    /v1/child-runs/:childId
POST   /v1/child-runs/:childId/abort
POST   /v1/child-runs/:childId/steer
POST   /v1/child-runs/:childId/resume
DELETE /v1/child-runs/:childId
GET    /v1/child-runs/:childId/thread
GET    /v1/child-runs/:childId/events
```

## 附录 B：事件类型全集（`kun/src/contracts/events.ts`）

46 个 `kind`：`thread_created`、`thread_updated`、`turn_started`、`turn_completed`、
`turn_failed`、`turn_aborted`、`turn_steered`、`item_created`、`item_updated`、
`item_completed`、`assistant_text_delta`、`assistant_reasoning_delta`、`tool_call_ready`、
`tool_dispatch_trace`、`tool_result_upload_wait`、`tool_storm_suppressed`、
`tool_catalog_changed`、`tool_call_started`、`tool_call_finished`、`approval_requested`、
`approval_resolved`、`user_input_requested`、`user_input_resolved`、`compaction_started`、
`compaction_completed`、`goal_updated`、`goal_cleared`、`todos_updated`、`todos_cleared`、
`pipeline_stage`、`context_pressure`、`bash_session_started`、`bash_session_updated`、
`bash_session_completed`、`usage`、`error`、`heartbeat`、`llm_span_start`、`llm_span_end`、
`fingerprint_drift`、`recovery_triggered`、`sse_idle_timeout`、`transient_retry`、`child_activity`

## 附录 C：内置工具清单（`kun/src/adapters/tool/builtin-tools.ts`）

`read`、`bash`、`edit`、`write`、`grep`、`find`、`ls`、`lsp`、`git_inspect`、`run_script`

## 附录 D：Runner 命令与事件协议（`packages/runner/src/protocol.ts`、`packages/contracts/src/runner-event.ts`）

命令：`startBrowser`、`focusBrowser`、`navigate`、`observe`、`verifyLocator`、
`verifyLocators`、`executeStepAction`、`executeBundle`、`pause`、`resume`、`cancel`、`shutdown`

事件（`runner-event/v1`）：`runStarted`、`stepStarted`、`log`、`stepFinished`、
`artifactCreated`、`runFinished`

定位策略：`testId`、`role`、`label`、`placeholder`、`text`、`css`、`xpath`

步骤动作：`assertURL`、`check`、`uncheck`、`assertVisible`、`assertText`、
`navigate`、`fill`、`click`

## 附录 E：`kun` → `ark` 更名映射（避免实施时歧义）

| 概念 | 旧 | 新 | 备注 |
|---|---|---|---|
| 产品/运行时名 | `kun` | `ark` | 文档与代码统一 |
| Node 后端二进制 | `kun serve`（`kun/dist/cli/serve-entry.js`） | `ark serve` | 迁移期二者并存，端口不同 |
| Rust 服务端二进制 | — | `arkd` | 组合根 |
| 设置键 | `agents.kun.*` | `agents.ark.*` | 读旧写新，见 §12.2-6 |
| 数据目录 | `~/.kun/data`（现 `~/.ark/data` 已有） | `~/.ark/data` | `import-jsonl` 负责搬迁 |
| 渲染层桥 | `window.kunGui` | `window.arkGui` | 一次性重命名，避免双名并存 |
| 后端端口 crate | — | `ark-ports`（含 `BackendProvider`） | 纯 trait，无实现 |
| 后端实现插件 | `studio-*` 里的 Galaxy 直连 | `plugins/plugin-backend-galaxy` / `plugin-backend-local` | 与其它插件同一 manifest/registry |
| 类型/字段 | `KunErrorCode` 等 | `ArkErrorCode` 等 | 生成物统一前缀 |
| 文档历史名词 | kun-mapper.ts 等 | 新代码中不再出现 | 旧文件名在迁移对照中保留 |

> 注意：`child-runs`、`threads` 等**磁盘目录名**与 **REST 路径**保持不变，
> 避免破坏现有数据与外部集成。

---
