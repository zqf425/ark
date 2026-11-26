/**
 * Trace analysis surface (`GET /v1/threads/{id}/trace-stats`, requirements/
 * trace-analysis.md §数据与投影).
 *
 * This file is the thin typed boundary for the settings trace section: every
 * wire shape is re-exported from the generated contract, and the only logic
 * here is the prompt composition the requirement assigns to the web api layer.
 */
import { ApiError, get, post } from '../../../shared/api/client'
import type { ThreadId, ThreadSummary, TurnSummary, Usage } from '../../../shared/contracts'
import type {
  ToolStat,
  TraceErrorEntry,
  TraceRuntimeError,
  TraceSlowCall,
  TraceSpan as TraceTimelineSpan,
  TraceSpanDetail,
  TraceSpanDetailKind,
  TraceSpanUsage,
  TraceSpanKind as TraceTimelineSpanKind,
  TraceStatsResponse,
  TraceTimeline,
  TurnStat
} from '../../../generated'
import {
  traceTokenCacheReported,
  traceTokenSegments,
  type TraceTokenSegment
} from './trace-token-segments'

export type { TraceTokenSegment }
export { traceTokenCacheReported, traceTokenSegments }

export type {
  ToolStat,
  TraceErrorEntry,
  TraceRuntimeError,
  TraceSlowCall,
  TraceStatsResponse,
  TraceTimeline,
  TraceSpanDetail,
  TraceSpanDetailKind,
  TraceSpanUsage,
  TraceTimelineSpan,
  TraceTimelineSpanKind,
  TurnStat
}

/** Session headline token counts, folded over every `usage` event. */
export type TraceTokenTotals = {
  prompt_tokens: number
  completion_tokens: number
  total_tokens: number
}

/**
 * Fold the optional top-level `usage` into the three plain counters the metric
 * cards render. The runtime may omit `usage` entirely (a thread that never
 * completed a model round), and `Usage.total_tokens` is authoritative when
 * present — the sum is only the display fallback for providers that skip the
 * total. Pure presentation arithmetic, not a second fold of state.
 */
export function traceTokenTotals(usage: Usage | undefined): TraceTokenTotals {
  const prompt = usage?.prompt_tokens ?? 0
  const completion = usage?.completion_tokens ?? 0
  return {
    prompt_tokens: prompt,
    completion_tokens: completion,
    total_tokens: usage?.total_tokens ?? prompt + completion
  }
}

/** `GET /v1/threads/{id}/trace-stats` — read-only projection, nothing persisted. */
export function fetchTraceStats(threadId: ThreadId, signal?: AbortSignal): Promise<TraceStatsResponse> {
  return get<TraceStatsResponse>(`/v1/threads/${threadId}/trace-stats`, signal ? { signal } : {})
}

/**
 * `GET /v1/threads/{id}/trace-timeline?turn_id=<id>` — single-turn readout.
 *
 * `404` is a **value**, not a failure: a turn from an extremely old log has no
 * timeline data, and the panel renders its empty state rather than an error.
 * Every other rejection keeps the shared `ApiError` so `describeError` stays
 * the single wording channel. The runtime path lives only here (api layer).
 */
export async function fetchTraceTimeline(
  threadId: ThreadId,
  turnId: string,
  signal?: AbortSignal
): Promise<TraceTimeline | null> {
  try {
    return await get<TraceTimeline>(
      `/v1/threads/${threadId}/trace-timeline`,
      signal ? { signal, query: { turn_id: turnId } } : { query: { turn_id: turnId } }
    )
  } catch (reason) {
    if (reason instanceof ApiError && reason.status === 404) return null
    throw reason
  }
}

/**
 * Thread rows for the picker, straight from the existing list query. The
 * stats endpoint needs no extra request parameters, so the trace surface
 * reuses `listThreads` instead of inventing a second path.
 */
export function listThreads(signal?: AbortSignal): Promise<ThreadSummary[]> {
  return get<ThreadSummary[]>('/v1/threads', signal ? { signal } : {})
}

/**
 * The analysis-session delivery, in order: create the session (title
 * `AI分析-<原线程标题>`, no workspace/runtime override — it must land in the
 * default project like a normal new session), then start one turn carrying
 * the prompt. Both POSTs live here and nowhere else, so a component can only
 * fire them together and the default state cannot reach either one.
 */
export async function createAnalysisThread(title: string): Promise<ThreadSummary> {
  return post<ThreadSummary>('/v1/threads', { title })
}

export function startAnalysisTurn(threadId: ThreadId, text: string): Promise<TurnSummary> {
  return post<TurnSummary>(`/v1/threads/${threadId}/turns`, { text, attachment_ids: [] })
}

/** JSON digest ceiling from the requirement: the stats summary inside one prompt. */
export const TRACE_PROMPT_JSON_LIMIT_BYTES = 8 * 1024

/**
 * UTF-8 byte length of a string (the cap the requirement states is bytes, not
 * code units, so a CJK-heavy digest truncates earlier than `.length` would).
 */
function utf8Length(value: string): number {
  return new TextEncoder().encode(value).length
}

/** Cut `json` to at most `limit` UTF-8 bytes without splitting a code point. */
function truncateUtf8(json: string, limit: number): string {
  if (utf8Length(json) <= limit) return json
  const bytes = new TextEncoder().encode(json)
  let end = limit
  // Back off until the slice boundary lands between code points.
  while (end > 0 && (bytes[end]! & 0b1100_0000) === 0b1000_0000) end -= 1
  return new TextDecoder().decode(bytes.slice(0, end))
}

/**
 * One-line session digest embedded above the JSON block, so the model gets the
 * headline numbers even when a long log pushes them out of the truncated JSON.
 * Pure string composition over the generated readout.
 */
function traceSummaryLine(stats: TraceStatsResponse): string {
  const tokens = traceTokenTotals(stats.usage)
  const tools = stats.tool_stats
    .slice(0, 8)
    .map((tool) => `${tool.tool}×${tool.calls}`)
    .join('、')
  return [
    `会话概要：回合数 ${stats.turns}，模型调用 ${stats.model_calls}，工具调用 ${stats.tool_calls}（失败 ${stats.failed_tool_calls}），`,
    `总时长 ${stats.active_ms ?? '未知'} ms，token ${tokens.total_tokens}，活跃天数 ${stats.active_days}。`,
    tools.length > 0 ? `工具分布：${tools}。` : '工具分布：无工具调用。',
    stats.errors.length > 0
      ? `错误 ${stats.errors.length} 条（含运行时错误），慢调用 ${stats.slowest_calls.length} 条。`
      : '错误清单为空。'
  ].join('')
}

/**
 * The controlled analysis prompt (requirement §AI 分析 prompt 契约): a fixed
 * Chinese template over four analysis dimensions with the stats JSON digest
 * capped at 8 KiB. Pure string composition — the model, not this page, runs
 * the analysis in the session the button creates.
 */
export function buildAnalysisPrompt(stats: TraceStatsResponse): string {
  const digest = truncateUtf8(JSON.stringify(stats, null, 2), TRACE_PROMPT_JSON_LIMIT_BYTES)
  return [
    '你是本机 ark 运行时的链路分析助手。下面是一次会话的 trace-stats 只读统计摘要，请基于它输出分析结论，',
    '并按需调用 workspace 工具回读该会话日志补充证据，不要臆造数据。分析聚焦四类维度：',
    '1. 报错：错误清单中的失败工具与运行时错误，指出根因与修复建议；',
    '2. 耗时较长操作：慢调用 Top 列表与逐回合耗时，指出可优化环节；',
    '3. 用户重复复述：同一诉求被多次重述的模式，说明可能的沟通或执行偏差；',
    '4. 多次执行才成功：同名工具反复调用或失败后重试的序列，给出改进建议。',
    '',
    traceSummaryLine(stats),
    '',
    'trace-stats JSON 摘要（超过 8KB 已截断）：',
    digest
  ].join('\n')
}

/**
 * Title for the analysis session: `AI分析-<原线程标题>` (requirement §设置 UI).
 * The fallback word is deliberately inline: this module is api-layer (no
 * React, no `translateNow`), and the title is a user-visible *artifact*
 * created by the button, not page copy.
 */
export function analysisThreadTitle(sourceTitle: string): string {
  const base = sourceTitle.trim().length > 0 ? sourceTitle.trim() : '未命名会话'
  return `AI分析-${base}`
}
