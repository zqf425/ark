/**
 * Settings -> 链路分析 (requirements/trace-analysis.md).
 *
 * Trace analysis renders a read-only stats projection plus one deliberate
 * trigger: the AI-analysis button is the only path that creates a session.
 * The default state never calls the model, so the copy always separates
 * "统计" (what happened) from "分析" (the session the user asks for).
 *
 * Identifiers (`turn`, `tool`, `token`) keep their wire spelling; prose is
 * translated on both columns. Cross-domain reuses: `common.*` for generic
 * buttons, `canvas.untitled` for a nameless thread, and the trace analysis
 * session title falls back to `thread.list.defaultTitle` when the source
 * thread has no title.
 */
export const SETTINGS_TRACE_MESSAGES = {
  'settings.trace.title': ['链路分析', 'Trace analysis'],
  'settings.trace.desc': [
    '查看会话的只读统计摘要；点击按钮才会创建 AI 分析会话。',
    'Read-only statistics for a session; the AI analysis session is only created on click.'
  ],
  'settings.trace.threadLabel': ['选择会话', 'Session'],
  'settings.trace.threadPlaceholder': ['选择要分析的会话…', 'Pick a session to analyze…'],
  'settings.trace.reload': ['重新统计', 'Re-run stats'],
  'settings.trace.loadFailed': ['统计加载失败', 'Failed to load trace statistics'],

  'settings.trace.emptyTitle': ['选择会话后查看链路统计', 'Pick a session to see its trace statistics'],
  'settings.trace.emptyHint': [
    '统计由会话事件日志投影而来，不会产生任何会话或模型调用。',
    'Statistics are projected from the session event log; no session or model call is created.'
  ],

  'settings.trace.metrics.turns': ['回合数', 'Turns'],
  'settings.trace.metrics.llmCalls': ['模型调用', 'Model calls'],
  'settings.trace.metrics.toolCalls': ['工具调用', 'Tool calls'],
  'settings.trace.metrics.toolFailures': ['失败数', 'Failures'],
  'settings.trace.metrics.activeMs': ['总时长', 'Active time'],
  'settings.trace.metrics.tokens': ['Token', 'Tokens'],
  'settings.trace.metrics.activeDays': ['活跃天数', 'Active days'],

  'settings.trace.chart.tools': ['工具调用分布', 'Tool call distribution'],
  'settings.trace.chart.turns': ['逐回合耗时', 'Active time per turn'],
  'settings.trace.chart.slow': ['慢调用 Top {count}', 'Slow calls top {count}'],
  'settings.trace.chart.errors': ['错误清单', 'Error list'],
  'settings.trace.chart.toolsEmpty': ['该会话没有工具调用记录。', 'This session recorded no tool calls.'],
  'settings.trace.chart.turnsEmpty': ['该会话还没有回合耗时数据。', 'This session has no per-turn timing yet.'],
  'settings.trace.chart.slowEmpty': ['没有可展示的慢调用。', 'No slow calls to show.'],
  'settings.trace.chart.errorsEmpty': ['该会话没有记录错误。', 'This session recorded no errors.'],
  'settings.trace.toolHint': ['调用 {calls} 次，失败 {failures} 次', '{calls} calls, {failures} failed'],
  'settings.trace.turnHint': [
    '回合 {index}：活跃 {duration}，模型 {rounds} 轮，工具 {calls} 次',
    'Turn {index}: active {duration}, {rounds} model rounds, {calls} tool calls'
  ],
  'settings.trace.turnsSummary': ['共 {count} 个回合，合计 {total}。', '{count} turns, {total} in total.'],

  'settings.trace.col.tool': ['工具', 'Tool'],
  'settings.trace.col.calls': ['调用', 'Calls'],
  'settings.trace.col.failures': ['失败', 'Failures'],
  'settings.trace.col.avgMs': ['平均耗时', 'Avg time'],
  'settings.trace.col.at': ['时间', 'Time'],
  'settings.trace.col.args': ['参数摘要', 'Argument digest'],
  'settings.trace.col.detail': ['详情', 'Detail'],
  'settings.trace.errorUnknown': ['（无失败原因）', '(no failure reason)'],
  'settings.trace.errorRuntime': ['运行时错误', 'Runtime error'],

  'settings.trace.analyze': ['启动 AI 分析', 'Start AI analysis'],
  'settings.trace.analyzing': ['正在创建分析会话…', 'Creating analysis session…'],
  'settings.trace.analyzeHint': [
    '创建一个新会话并发送统计摘要，由模型按需回读原会话日志；本页不会自动开始分析。',
    'Creates a new session seeded with the stats digest; the model reads the source log on demand. This page never starts one automatically.'
  ],
  'settings.trace.analyzeSucceeded': ['分析会话已创建：{title}', 'Analysis session created: {title}'],
  'settings.trace.analyzeFailed': ['启动 AI 分析失败', 'Failed to start AI analysis']
} as const
