# 设置内链路分析(trace analysis)

> 状态:已批准计划(基于 smart-ark ark 模式同名功能的 ark 原生移植;LLM span 事件为后续里程碑,首版以既有事件投影)。
>
> v2(2026-11-26):新增耗时下钻——单回合时间线端点 + 图表行点击展开(见 §v2)。

## 背景与目标

smart-ark 的 ark 模式在「系统管理 → 链路分析」提供会话级统计图表 + 可选 AI 分析
(`src/renderer/src/components/ark/observability/TraceAnalysisPage.tsx`、主进程
`thread-stats.ts`)。ark 侧契约已预留位置:`EventKind::LlmSpanStart/LlmSpanEnd`
在 `PLANNED_KINDS`(声明无生产者),诊断 handler(`thread_usage`/`llm-rounds`)
已从事件日志聚合。本需求把该能力以 ark 原生形态落在 arkd + web 设置页。

**核心交互约束(用户指定)**:设置新增「链路分析」section,**默认只展示图表,
不启动 AI 分析**;图表下方提供「启动 AI 分析」按钮,点击才创建分析会话。

## 数据与投影(arkd 侧)

新增 `GET /v1/threads/{id}/trace-stats`,从该线程事件日志(replay_events)
投影只读统计,不落盘、不存第二份状态(与 diagnostics.rs 同一模式):

- `trace_stats.rs`(ark-server `api/handlers/`):纯函数 `aggregate_trace_stats(thread_id, events) -> TraceStatsResponse`。
- 契约类型加在 `ark-contract`(`TraceStatsResponse` / `TurnStat` / `ToolStat` 等),
  ts-rs 生成 TS,跑 `cargo test -p ark-contract --lib ts_export` + `git diff --exit-code web/src/generated`。
- 统计维度(全部可从现有事件推导,无需新埋点):
  - 会话级:回合数、总时长(首 turn_started 末 turn 终止时间差)、模型调用次数
    (usage 事件计数)、token 用量(usage 折叠)、工具调用总数/失败数
    (tool_call_finished 的 ok=false)、活跃会话天数。
  - 逐回合:每 turn 的 active_ms(复用 TurnUsage 折叠语义)、模型轮数、
    工具调用数、失败工具数。
  - 工具分布:按 tool 名聚合调用次数/失败次数/平均耗时。
  - 慢调用 Top N:按 duration_ms 排序的 tool_call_finished(截断到 N=10,
    附 call_id/tool/参数摘要/耗时)。
  - 错误清单:ok=false 的调用 + 同窗口 RuntimeEvent::Error,各带时间戳。
- LlmSpanStart/LlmSpanEnd 保持 PLANNED 不动;usage 事件已带每模型轮的
  prompt/completion/cache 计数,统计语义够首版使用。契约注释注明这一取舍。

## 路由与安全

- 路由挂 `ark-server` routes.rs,与 `/v1/threads/{id}/usage` 同层。
- 只读 GET;`limit`/参数无需(按单线程全量 replay,与 thread_usage 相同边界)。
- 不新增写路径,不触碰 workspace_gate。

## 设置 UI(web 侧)

- `settings-navigation.ts` 加 `trace` section(组:capabilities,icon: `chart`),
  i18n 键入 `settings-core.ts`(`settings.nav.trace.label/detail`,中英双语)。
- 新组件 `TraceSection.tsx`(features/settings/components/):
  - 顶部线程选择器(复用 thread-list 的查询 API,展示标题+更新时间)。
  - 选中后调 `/v1/threads/{id}/trace-stats` 渲染图表:核心指标卡片、
    工具调用分布(横向条形)、逐回合耗时(条形)、错误/慢调用清单(表格)。
    图表用内联 SVG + 现有 ds-* 设计变量,**不引入图表库**(与 AGENTS.md
    最小依赖一致;smart-ark 用 recharts,ark 侧手工 SVG 足够且无新依赖)。
  - 底部「启动 AI 分析」按钮:仅当已加载统计后可用。点击创建分析会话
    (POST /v1/threads,标题 `AI分析-<原线程标题>`),发送一条受控 prompt,
    把本页统计摘要嵌入 prompt 正文,由模型自行调用 workspace 工具读取原线程
    日志(如需)。分析会话在会话画布打开,不内嵌到设置页。
  - **默认态:只渲染图表,不创建任何会话、不调用模型**;按钮是唯一触发点。
- API 层:仅 `features/settings/api/` 下新增 `trace-api.ts`(规则:运行时路径
  只出现在 api 层,check-hard-rules 会拦)。
- 文件行数:组件保持 ≤1000 行,必要时拆 `TraceCharts.tsx`。

## AI 分析 prompt 契约

固定模板(中文),四类分析维度与 smart-ark 对齐:报错、耗时较长操作、
用户重复复述、多次执行才成功。把 trace-stats 的 JSON 摘要(截断到 8KB)
注入 prompt;会话正文不重复注入,由模型按需取原线程 detail。模板字符串
宿主在 Rust(ark-server handler 或前端 api 层均可,定为前端 api 层,理由:
模板属于 web 域的展示组合,非 wire 契约)。

## 不做的事(非目标)

- 不实现 LlmSpanStart/LlmSpanEnd 生产者(独立里程碑,涉及 agent loop 埋点)。
- 不做跨线程汇总报表、不做导出 CSV/PDF。
- 不改 session canvas、不动 gate/审批语义。

## 验收

- 单测(Rust):`aggregate_trace_stats` 纯函数覆盖——零事件线程、正常混合、
  失败工具、慢调用排序截断、Error 事件关联。
- 契约测试:`cargo test -p ark-contract --lib ts_export` 生成新 TS 类型,
  `git diff --exit-code web/src/generated` 通过(生成后提交)。
- web 单测:TraceSection 默认态不发分析请求(按钮未点时零 createThread 调用);
  线程选择器加载;图表对空数据渲染空态。
- check-hard-rules 通过(≤1000 行、api 层规则)。
- i18n:catalog-completeness 测试通过(中英齐全)。
- 手动验证:`cargo run -p arkd -- serve` + web dev,选一个有工具调用的线程
  看图表;点「启动 AI 分析」生成新会话;刷新后图表仍在、无自动分析。

## v2:耗时下钻(单回合时间线)

对齐 smart-ark ThreadStatsPanel 的下钻能力(工具明细/逐轮时间线/异常详情),
v1 只给了聚合数字与 Top 清单,无法回答"这个回合的 2 分钟花在哪"。

### 端点与契约

- `GET /v1/threads/{id}/trace-timeline?turn_id=<id>`:单回合、有界读侧
  (未知 turn_id 返回 404,与 parse_thread 语义一致)。
- 契约类型 `TraceTimeline`(ark-contract,ts-rs 生成),核心是按
  `created_at` 升序的 span 列表;每个 span:
  - `kind`:`user_message | assistant_text | reasoning | tool_call |
    approval | user_input | compaction | model_call | error | notice`
    (对齐 `ItemKind` + `model_call` 来自 usage 事件)。
  - `started_at`/`duration_ms`:工具与模型调用有真实耗时
    (`ToolPayload.duration_ms`、`Usage.duration_ms`);文本类 item 只有
    created_at,`duration_ms` 为 None(契约注释注明)。
  - `label`(工具名/模型名)、`text_snippet`(截断 200 字)、
    `is_error`、`item_id`(可跳回会话画布)。
  - `model_call` 行另带可选 `usage`:单次 `usage` 事件的 token/缓存快照
    (`TraceSpanUsage`,逐字段搬运,不聚合);缓存字段是 provider 可选上报,
    缺失为 `None`,不得推算或伪造。
- 折叠规则:thread detail 同源——`ItemCreated/Updated/Completed` 按
  item_id 取最后版本(与 store 投影一致,避免流式多版本重复);
  `Usage` 事件生成 `model_call` span;`RuntimeEvent::Error` 生成
  error span。LLM 精确计时仍受 `LlmSpanStart/End` 未实现限制,
  `model_call` 用 usage 事件墙钟近似——契约注释如实声明。
- 事件正文不进时间线(原文太长),文本只留 200 字摘要;要看全文走
  既有 thread detail。
- `reasoning` / `assistant_message` item 的 `created_at` 落在模型调用
  `[end-duration, end]` 窗口内,是模型调用期间流式产出的子事件,不是独立
  占用时间的活动:它们不再产出独立甘特行,而是作为 `details`(`kind` +
  `text_snippet` + `started_at`)挂到所属 `model_call` 行,同类多段全部保留、
  按 `started_at` 升序。归属规则:选包含该 `created_at` 且时长最短的模型调用
  窗口(区间两端闭合);找不到窗口的片段(旧日志无 `duration_ms`)直接丢弃,
  全文仍可从 thread detail 读取。契约注释同步说明该丢失路径。

### web 交互

- 图表行可点击:逐回合耗时条形(TraceCharts)每行 → 展开该回合
  时间线面板;慢调用/错误表格行 → 定位到所属回合并展开。
- 时间线面板:垂直 span 列表(色块 = kind,右侧耗时,failed 红标),
  加载态与空态;不新增依赖,不新增 npm 包。
- 仅在用户点击时请求该端点(每回合一次,无预取)。

### v2.1:模型调用行的 token / 缓存指标(当前实现)

- `TraceSpan` 的 `model_call` 行携带可选 `usage`(`TraceSpanUsage`,镜像单次
  `ark_common::Usage` wire 字段):`prompt_tokens`/`completion_tokens`/`total_tokens`
  必有;`cache_hit_tokens`/`cache_miss_tokens`/`cache_hit_rate` 等可选,
  `cache_hit_rate_approximate` 由后端标记命中率是否为推断值。前端不做除法,
  也不把缺失的缓存字段补成 0。
- 展开详情时,仅 `model_call` 行渲染「Token 用量」与「缓存指标」两个区块:
  输入/输出/合计 token 用 `Intl.NumberFormat(locale)` 千分位;缓存字段只显示
  provider 实际给出的项,命中率用百分比格式(一位小数),近似值按标记加
  「约」。三项缓存字段全缺时显示「供应商未上报缓存统计」;未展开与非模型
  行均不渲染这两个区块。
- **当前实现**:Rust 侧 `TraceSpanUsage` 已登记 ts_export 根并生成
  `web/src/generated/TraceSpanUsage.ts`,`TraceSpan.usage` 已出现在生成类型;
  `features/settings/api/trace-api.ts` 统一 re-export,组件与测试只经 api 层消费。
- 文案全部登记 `settings-trace.ts` 中英双语,随 catalog-completeness 校验。
