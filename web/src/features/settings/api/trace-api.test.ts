/**
 * The analysis prompt composer (requirement §AI 分析 prompt 契约).
 *
 * Pinned: the four analysis dimensions are always present, the stats JSON
 * digest is embedded together with a human-readable headline line, the 8 KiB
 * ceiling cuts by UTF-8 bytes without splitting a code point, and the
 * analysis session title wraps the source title.
 */
import { describe, expect, it } from 'vitest'
import type { Usage } from '../../../shared/contracts'
import {
  analysisThreadTitle,
  buildAnalysisPrompt,
  traceTokenCacheReported,
  traceTokenSegments,
  traceTokenTotals,
  TRACE_PROMPT_JSON_LIMIT_BYTES,
  type TraceStatsResponse
} from './trace-api'

function usage(overrides: Partial<Usage> = {}): Usage {
  return { prompt_tokens: 1_000, completion_tokens: 500, total_tokens: 1_500, ...overrides }
}

function stats(overrides: Partial<TraceStatsResponse> = {}): TraceStatsResponse {
  return {
    thread_id: '00000000-0000-0000-0000-000000000001',
    turns: 3,
    active_ms: 120_000,
    model_calls: 7,
    usage: usage(),
    tool_calls: 5,
    failed_tool_calls: 1,
    active_days: 2,
    turn_stats: [],
    tool_stats: [{ tool: 'read_file', calls: 4, failures: 0 }],
    slowest_calls: [],
    errors: [],
    ...overrides
  }
}

describe('buildAnalysisPrompt', () => {
  it('names all four analysis dimensions', () => {
    const prompt = buildAnalysisPrompt(stats())
    expect(prompt).toContain('报错')
    expect(prompt).toContain('耗时较长操作')
    expect(prompt).toContain('用户重复复述')
    expect(prompt).toContain('多次执行才成功')
  })

  it('embeds a JSON digest of the stats and a headline summary', () => {
    const prompt = buildAnalysisPrompt(stats())
    expect(prompt).toContain('"thread_id"')
    expect(prompt).toContain('"failed_tool_calls": 1')
    expect(prompt).toContain('回合数 3')
    expect(prompt).toContain('模型调用 7')
  })

  it('falls back to the token sum when the usage total is missing', () => {
    const prompt = buildAnalysisPrompt(stats({ usage: usage({ total_tokens: undefined }) }))
    expect(prompt).toContain('token 1500')
  })

  it('truncates the digest to the 8 KiB byte budget', () => {
    // 9 KiB of ASCII inside one field blows past the cap by bytes, not chars.
    const big = stats({
      errors: [
        {
          seq: 1,
          turn_id: '00000000-0000-0000-0000-000000000002',
          call_id: 'c0',
          tool: 'exec',
          timestamp: '2026-11-02T00:00:00Z',
          duration_ms: 10,
          detail: 'x'.repeat(9 * 1024),
          errors: []
        }
      ]
    })
    const prompt = buildAnalysisPrompt(big)
    const digestStart = prompt.indexOf('trace-stats JSON 摘要')
    const digest = prompt.slice(digestStart)
    // The cut happened: the full 9 KiB payload is not present, and the total
    // prompt stays on the order of the cap (template + digest + headline).
    expect(digest).not.toContain('x'.repeat(9 * 1024))
    expect(new TextEncoder().encode(prompt).length).toBeLessThan(TRACE_PROMPT_JSON_LIMIT_BYTES + 2_000)
  })

  it('does not split a code point when truncating multibyte content', () => {
    // 4096 CJK characters = 12 KiB of UTF-8; the cut must land between chars.
    const big = stats({
      errors: [
        {
          seq: 1,
          turn_id: '00000000-0000-0000-0000-000000000002',
          call_id: 'c0',
          tool: 'exec',
          timestamp: '2026-11-02T00:00:00Z',
          duration_ms: 10,
          detail: '错'.repeat(4096),
          errors: []
        }
      ]
    })
    const prompt = buildAnalysisPrompt(big)
    expect(prompt).not.toContain('\uFFFD')
    expect(prompt.length).toBeGreaterThan(0)
  })
})

describe('traceTokenTotals', () => {
  it('keeps the authoritative total when the usage carries one', () => {
    expect(traceTokenTotals(usage())).toEqual({ prompt_tokens: 1_000, completion_tokens: 500, total_tokens: 1_500 })
  })

  it('falls back to the sum and to zeros for a missing usage', () => {
    expect(traceTokenTotals(usage({ total_tokens: undefined })).total_tokens).toBe(1_500)
    expect(traceTokenTotals(undefined)).toEqual({ prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 })
  })
})

describe('traceTokenSegments', () => {
  it('splits prompt into exclusive input-miss/cache-hit shares and keeps the output share', () => {
    const segments = traceTokenSegments(usage({ cache_hit_tokens: 600, cache_miss_tokens: 400 }))
    expect(segments).toEqual([
      { key: 'uncached', labelKey: 'settings.trace.gantt.tokenUncached', value: 400 },
      { key: 'hit', labelKey: 'settings.trace.gantt.tokenCached', value: 600 },
      { key: 'output', labelKey: 'settings.trace.gantt.tokenOutput', value: 500 }
    ])
    expect(segments.reduce((sum, segment) => sum + segment.value, 0)).toBe(1_500)
  })

  it('caps a reported hit count at prompt so segments never overlap', () => {
    // A provider may report cache hits beyond the prompt total; the excess
    // must not turn the shares into a double count.
    expect(traceTokenSegments(usage({ cache_hit_tokens: 4_000 })).map((segment) => [segment.key, segment.value]))
      .toEqual([['hit', 1_000], ['output', 500]])
  })

  it('renders only input/output when no cache field is reported', () => {
    expect(traceTokenCacheReported(usage())).toBe(false)
    expect(traceTokenSegments(usage())).toEqual([
      { key: 'uncached', labelKey: 'settings.trace.gantt.tokenUncached', value: 1_000 },
      { key: 'output', labelKey: 'settings.trace.gantt.tokenOutput', value: 500 }
    ])
    expect(traceTokenCacheReported(usage({ cache_miss_tokens: 100 }))).toBe(true)
    expect(traceTokenCacheReported(usage({ cache_hit_rate: 0.2 }))).toBe(true)
    expect(traceTokenCacheReported(undefined)).toBe(false)
  })

  it('clamps negative wire counters and handles an absent usage', () => {
    expect(traceTokenSegments(usage({ prompt_tokens: -10, completion_tokens: -5, cache_hit_tokens: 3 }))).toEqual([])
    expect(traceTokenSegments(undefined)).toEqual([])
  })
})

describe('analysisThreadTitle', () => {
  it('wraps the source title', () => {
    expect(analysisThreadTitle('修复登录')).toBe('AI分析-修复登录')
  })

  it('trims whitespace and falls back for an empty title', () => {
    expect(analysisThreadTitle('  深入分析  ')).toBe('AI分析-深入分析')
    expect(analysisThreadTitle('   ')).toBe('AI分析-未命名会话')
  })
})
