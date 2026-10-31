# 发布收尾与真实环境验收

> 状态：**部分完成**。真实浏览器交互、全仓 Clippy 与统一验证已闭环（见「验收记录」）；
> 桌面 sidecar 的「合法复用」与「复用模式退出收敛」已有真实证据；
> 「空闲端口冷启动」与「未知服务占用端口的冲突边界」因用户自有 `arkd` 长期占用
> `127.0.0.1:18791` 而阻塞，按第 2 节不得停掉用户进程制造条件，故不宣称完成。
> 尚需真实模型行为才能触发的交互（steer/中断、审批与输入闸门、子代理全生命周期、
> 自动续跑批次、四个右侧面板、会话级覆盖）如实标注为未覆盖，见「验收记录 D」。
> 本需求只承接真实交互、桌面冷启动与全仓质量问题，不重复已经完成的功能开发。

## 目标

在真实浏览器和 Windows 桌面环境中验证当前 `master` 的用户路径，修复验收暴露的产品缺陷，并将正式需求状态更新到与证据一致。任何修复都必须遵守独立 worktree、最小改动、定向验证和提交后合入流程。

## 执行顺序

### 1. Code、设置与自动续跑的真实交互验收

使用独立 `arkd`、独立数据目录和不占用用户进程的端口启动 Web，逐项验证并留存可复现的操作记录：

- 线程新建、选择、重命名、归档/恢复、删除与 fork；rewind 目前只有服务端实现，web 无入口，不在本清单内；
- 普通回合发送、运行中 steer、中断，以及审批和用户输入 gate 的成功、重复、过期和取消路径；
- 子代理前台/后台启动、排队、完成、失败、abort、steer、resume、摘要跳转和用量展示；
- `subagents.auto_continue` 默认关闭时不产生额外模型调用；显式开启后，多个完成通知合并、每批最多 8 条、正常确认后继续排空，中断/归档/删除/关闭配置后不再自动续跑；
- Plan、Changes、File、Terminal 的真实数据、空态、错误态与刷新恢复；
- runtime tuning、会话级 workspace/provider/model/权限覆盖、provider 模型目录、MCP、Skills、语言/字号/主题的保存、生效和失败提示；
- SSE 断线重连、页面刷新和线程切换后，活动回合及待处理 gate 与服务端状态一致。

验收不得只依赖组件测试或静态截图。每个失败必须记录步骤、期望、实际结果和证据；确认是产品缺陷后先补回归测试，再做最小修复。

### 2. Windows 桌面 sidecar 冷启动验收

仅在 `127.0.0.1:18791` 空闲时执行；不得停止或接管用户已有的 `arkd` 来制造条件。

- 从桌面开发入口启动真实 WebView2 窗口，确认宿主自行启动专属 `arkd`；
- 确认 health、参数、工作区和数据目录均属于本次桌面实例；
- 退出窗口后确认由宿主持有的 sidecar 和进程树按设计收敛；
- 再验证已有合法 `arkd` 的安全复用，以及未知服务占用端口时只报冲突、不终止对方；
- 若桌面代码发生修复，重新执行桌面 Rust check/test、Node 布局测试、生产 Web 构建；影响打包路径时重新产出 NSIS 安装包。

环境暂不满足时必须如实记录阻塞，不得用独立端口启动普通 `arkd` 冒充桌面宿主冷启动证据。

### 3. 全仓质量收尾

- 运行统一验证入口的 core、contract、web、desktop scope；
- 运行 `cargo clippy --workspace --all-targets --exclude ark-desktop -- -D warnings`，修复当前所有可归因的 lint，不再以“既有问题”长期豁免；
- 保证 Rust 文件不超过 800 行、无 glob re-export、TypeScript 生成物无漂移、`git diff --check` 通过；
- 不为消除 warning 扩大产品行为或重构无关模块。

## 缺陷处理与独立检视闭环

执行会话发现产品、测试、脚本或文档问题后，必须通过 DSH 的 headless 新会话实施修复，不能把修复任务改派给当前会话的子代理。DSH 必须在该任务的独立 worktree 中工作，遵循 `AGENTS.md`，先写失败测试，再做最小修复；主执行会话负责核对 diff 和运行验证。

每轮 DSH 修复完成后，必须使用 Claude CLI 对完整未提交 diff 和相关代码做只读检视，并明确按 P0、P1、P2 分类。若仍有任何 P0 或 P1，主执行会话必须把原始意见交回 DSH 修复，再验证、再调用 Claude；循环直到 Claude 明确确认 **P0=0、P1=0**。P2 可在不影响验收正确性时记录为后续项，但不得把真实正确性或安全问题降级规避。

DSH 和 Claude 都不是验证真源。最终结论必须同时具备实际命令退出码、真实交互证据和主执行会话对 diff 的复核。

## 验收记录

本节只记录已实际执行的证据，每条均按所列命令或步骤可复现。临时日志与截图位于 `.tmp/`，不入库。

### A. 全仓质量收尾

| 项目 | 结果 | 证据 |
| --- | --- | --- |
| `scripts/verify.ps1 -Scope all` | 通过（exit 0） | `.tmp/final-verify2.log`：`VERIFY_ALL_EXIT=0` |
| `cargo clippy --workspace --all-targets --exclude ark-desktop -- -D warnings` | 通过（exit 0） | 同上：`CLIPPY_EXIT=0` |
| 架构与规范检查 | 通过 | 同上：`Architecture checks passed.` |

全仓 Clippy 整改由 DSH 在 `.worktrees/release-clippy-fix`（分支 `fix/release-clippy`）分轮完成，
覆盖 `ark-obs`、`ark-config`、`ark-ports`、`ark-agent`、`ark-http`、`ark-llm`、`ark-mcp`、`ark-store`、
`ark-tool`、`ark-delegation`、`ark-skills`、`ark-provider` 与 `apps/arkd`；
修复提交 `b9ec1aa`，合入 `a06ba4e`。未以“既有问题”豁免任何告警。

### B. Code 与设置的真实浏览器验收

统一以独立实例执行：独立 `arkd` + 独立数据目录，端口不占用用户进程；
Playwright Chromium 驱动真实页面，结论取自真实 HTTP 状态码与真实 UI 文本。

| 场景 | 结果 | 证据文件（`.tmp/`） |
| --- | --- | --- |
| 线程新建/重命名/fork/归档/恢复/刷新恢复 | 通过（200/201） | `release-browser-run4/EVIDENCE-thread-management.md` |
| 普通回合发送与助手回复收敛 | 通过 | `release-browser-run3/EVIDENCE-turn-send.md` |
| SSE 空 backlog 首帧 | 通过（修复后首帧 2.4ms；修复前 75s 零字节） | `release-browser-run3/EVIDENCE-sse-first-frame.md` |
| 空闲线程不再周期性误重连 | 通过（修复后 95s 内订阅数恒为 1） | `release-browser-run3/EVIDENCE-idle-reconnect-fixed.md` |
| 设置页 runtime tuning 与子 agent 投影 | 通过 | `release-browser-run3/EVIDENCE-settings-runtime.md` |
| `subagents.auto_continue` 默认关闭与开关往返 | 通过（含服务端热更新） | `release-browser-run3/EVIDENCE-auto-continue-toggle.md` |
| provider / Skills / MCP 真实数据与空态 | 通过 | `release-browser-run3/EVIDENCE-settings-{providers-empty,skills,mcp}.md` |

期间发现并修复的两个产品缺陷（均经 DSH 修复、回归测试与 Claude 只读检视闭环）：

1. **SSE 空 backlog 不发送首帧**：新线程订阅后 75 秒零字节，UI 永久停在“连接中”、发送按钮禁用。
   修复提交 `3ce6919`，合入 `9365394`；新增 `web/src/shared/api/client.test.ts` 与
   `web/src/features/code-session/api/events-api.test.ts` 共 318 行回归用例。
2. **SSE 空闲看门狗误判重连**：keep-alive 注释帧不产生 `frame`，无法重置 75s 看门狗，空闲线程被周期性
   abort 重连。修复提交 `5553eae`，合入 `eb4cb2f`，改为按字节判活。

### C. Windows 桌面 sidecar

| 场景 | 结果 | 证据文件（`.tmp/`） |
| --- | --- | --- |
| 真实 WebView2 窗口打开 `ark · Code` | 通过 | `release-browser-run3/EVIDENCE-desktop-blocker.md` 第 1 节 |
| 已有合法 `arkd` 的安全复用 | 通过 | `release-browser-run3/EVIDENCE-desktop-reuse-and-exit.md` 第 2、3 节 |
| 复用模式下宿主退出收敛 | 通过 | 同上第 4 节 |
| 空闲 18791 上自起专属 sidecar 冷启动 | **外部阻塞** | `release-browser-run3/EVIDENCE-desktop-blocker.md` 第 2 节 |
| 未知服务占用端口的冲突边界 | **外部阻塞** | 同上第 2 节 |

复用验收的关键断言（三者相互独立，缺一不可）：宿主 `ark-desktop.exe` 下**没有** `arkd` 子进程；
18791 的 LISTENING owner 仍是用户既有 PID；全程 `arkd` 实例总数不变。
退出收敛断言：关闭窗口后宿主退出，用户 `arkd` 的 `/health` uptime 继续增长（未被误杀、未重启）。

阻塞的可复现条件：`netstat -ano` 显示 `127.0.0.1:18791` 由用户真实桌面会话的 `arkd` 持有，
其 `--data-dir` 为 `%APPDATA%/com.ark.desktop/runtime`，启动时间早于本会话。
解除阻塞后的复现步骤见 `EVIDENCE-desktop-blocker.md` 第 2 节。

### D. 仍未覆盖的真实交互（不宣称通过）

> macOS 真实模型交互补充验收（2026-10-31，arkd @ master 8ff1afa，独立端口 18793 与独立数据目录，
> 真实网关 provider `openai_chat`，证据索引 `.tmp/accept-run1/EVIDENCE-d1-d2.md`，不入库）：

已通过（HTTP 状态码与权威 events.jsonl 双重证据）：

- 普通回合 + 真实 bash 工具链（审批后执行、输出回读）；运行中 steer（`turn_steered` 持久化、
  steered 文本以 user source 进入模型上下文）与 interrupt（202 -> 唯一终态 `turn_aborted`，
  挂起 gate 一并清理）；
- 审批 gate：成功（204 -> 工具执行）、重复（404 not pending）、拒绝（204 -> tool 失败结果回给
  模型）三条路径；
- 用户输入 gate：真实模型调用 `request_user_input` -> pending_user_inputs 投影 -> 回答 204 ->
  答案作为 tool result 回给模型；重复 404。取消路径复用 interrupt 机制（同一 oneshot 语义已验证）；
- 子代理后台启动与完成通知：后台 delegate 即返、终态 `child_completed` 投影、`auto_continue`
  单子代理场景 1 续跑 + 1 确认收敛；
- 四面板真实数据：Terminal（bash 转录含命令/退出状态/耗时）、File（已读文件受限预览、未读 404、
  缺 path 参数 400）、Changes（write 操作投影）、Plan（goal/todos POST+GET 投影）。

验收发现并已闭环的缺陷（均经 DSH 修复、独立复验后合入 master）：

1. P0 工具 schema wire 形状错误：`schemas()` 裸 schema 导致 openai_chat 网关静默忽略 tools，
   模型收不到任何工具定义。修复 ca27651，合入 4b08935。
2. P1 用户输入 gate 生产触发路径缺失：`request_user_input` 未注册、`Action::AskUser` 无生产
   构造点。修复 509d603（含 record_assistant_turn 结构化 tool_calls 化），合入 31913cc。
3. P0 auto_continue 无界续跑：失败通知永不确认 + 每次续跑重新委派，实测 10 分钟 9 回合 8 子代理。
   修复 6f21c69（同通知尝试上限 2、连续模型失败熔断暂停，合入 8ff1afa），复测确认单子代理
   1 续跑收敛。
4. P1（并行会话修复，53f3e7f）tool-call 历史回放配对断裂。

> macOS 真实模型交互补充验收 run2（2026-10-31，arkd @ master 44d87e5，独立端口 18800 与独立
> 数据目录，真实网关 `cn:glm-5.3-flash`，证据索引 `.tmp/accept-run2/REPORT.md`，不入库）：
> auto_continue 多通知排空、子代理生命周期时序、会话级覆盖均**通过**（HTTP 状态码与
> events.jsonl 双重证据）；审批 gate 结论为**无 TTL/过期语义**（仅 pending/approved/denied，
> 无 expires_at 与超时扫描），"过期路径"从清单移除，如需 TTL 另行立项。P0=0、P1=0。

run2 发现的 P2/P3（未修复，记录为后续项）：

1. P2 部分自动续跑回合在父线程 events.jsonl 缺失 `turn_started/turn_completed` 生命周期记录
   （疑似与 503 失败回合的记录路径有关），影响审计回放完整性，不影响续跑功能。
2. P2（证据局限）每批 8 条通知上限未获运行时实测（单批峰值 6，受 `max_concurrency=2` 限制），
   现有证据为 `MAX_COMPLETIONS_PER_TURN=8` 代码与单测。
3. P3（体验项）`GET /v1/threads/{id}` 不回显会话级 autonomy/workspace 覆盖；PATCH 传 `null`
   无法清除已设覆盖（需接口语义明示）。

> macOS 真实模型交互补充验收 run3（2026-11-01，arkd @ master 02954cb，独立端口 18797 与独立
> 数据目录，真实网关 `global:deepseek-v4.1-flash`，Playwright Chromium 驱动真实页面，
> 证据为真实 DOM 快照与 HTTP 状态码，不入库）：
>
> 已通过：web rewind 入口全链路（确认弹窗三段文案、回退后画布重建、seq 水位 121→122、
> 成功 toast、SSE 保持连接、服务端 items=0 与 last_seq 一致、工作区文件未动）；失败回合的
> 错误展示（错误码+原文）；运行中回合 rewind 入口正确禁用；子代理面板（完成卡片、用量
> "5 次工具调用"、活跃计数、补充指令入口、父回合审批卡）；语言切换即时生效
> （`html lang` en-US↔zh-CN）；英文模式设置页（工作区/供应商/集成/通用）目视零中文残留；
> 503 错误经 UI 如实呈现且可重试。
>
> 过程中发现并已闭环的缺陷（均经 DSH 修复、独立复验后合入 master）：
> 1. P1 `HistoryHygiene::repair` 误删携带 `tool_calls` 的空 assistant 行（deepseek 类模型
>    必现，工具结果对模型不可见并触发自我更正）。修复 c459a31，抓包复验通过。
> 2. P2 设置页"通用"区块与顶部状态栏硬编码中文。修复 6d93091。
> 3. P2 全 web 用户可见文案 i18n 债务（57 文件、532 键、非注释中文清零，含双语完备静态测试）。
>    修复 a2d92f3。
> 4. P2 `ThreadDetail.child_ids` 同一子代理重复出现（runs.jsonl 一行=一次状态转换，投影未
>    折叠）。修复 02954cb，统一 `ark_ports::latest_child_runs` 折叠语义。
> 5. P2 部分自动续跑回合缺失 turn_started/turn_completed 生命周期记录。修复 4e7db53
>    （supervisor 收敛时补写缺失的 start 记录），run2 缺陷 D1 关闭。
> 6. P3 PATCH/GET 覆盖语义未明示。run2 的"GET 不回显"前提在当前代码上不成立（实测已回显）；
>    修复 ebadf21 以契约注释+测试固化 PATCH 合并语义与 GET 回显行为，run2 缺陷 D3 关闭。

> macOS 补充验收 run4（2026-11-01，arkd @ master 1ca6625，独立端口 18801，真实网关
> `global:deepseek-v4.1-flash`，证据索引 `.tmp/accept-batch8/`，不入库）：
>
> **每批 8 条通知上限的运行时实测通过**：单回合委派 10 个后台子代理，首批起始时 9 条就绪
> 实际只消费 8 条（模型回显明确收到 10 个中的 8 个），批次序列 `[0, 8, 2]`，10/10 确认、
> 无丢失无重复；并发闸门实测峰值与 `max_concurrency` 一致。run2 的"8 条上限证据局限"P2 关闭。
>
> run4 新发现的观察项（未修复，记录为后续项）：
> 1. P2 `subagents.max_concurrency` 硬上限 8（`SUBAGENT_CONCURRENCY_MAX`，"ceiling is
>    enforced rather than advisory"），简报假设可配 ≥10 不可达；属设计约束，如需更高并发
>    另行立项。
> 2. P3 GUI 写出的 `config.json` 与 arkd `RuntimeConfig` 结构不兼容时**静默回落默认值**
>    （`#[serde(default)]` 且无 `deny_unknown_fields`、无告警），`auto_continue` 等关键开关
>    可能无声失效。建议加载时对未知键/回落行为打 warn 日志。
> 3. P3（环境）上游网关在多任务并发时间歇 503，子代理可能大面积以 `ModelRequestFailed`
>    结束；熔断按设计暂停续跑、通知不丢（run1/观察项 O1 佐证）。

仍未覆盖（延续原清单，不宣称通过）：

- 语言/字号/主题的"保存失败提示"路径（保存成功路径 run3 已验）；
- Windows 桌面端整体复验（保持既有外部阻塞：端口 18791 被用户进程占用）。

## 完成条件

1. Code/设置/自动续跑的上述真实交互全部通过，或每个外部阻塞都有可验证的环境说明和后续复现方式。
2. 桌面冷启动、退出收敛、合法复用和端口冲突边界均有真实证据；无法执行的部分不宣称完成。
3. 所有验收发现的缺陷均经 DSH 修复、定向回归和 Claude 循环检视，最终 P0/P1 为零。
4. 统一验证与 Clippy 通过，或仅剩无法由代码修复的明确外部阻塞。
5. 更新 `code-settings-ui.md`、`desktop-shell.md` 及受影响模块设计，使状态和证据一致；临时日志、截图索引、交接记录与 AI 检视报告只放 `.tmp/`。
6. 提交信息使用简体中文并合入 `master`；不得提交 `.tmp/`、构建产物或用户本地目录。
