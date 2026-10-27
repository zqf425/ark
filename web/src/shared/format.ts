/** Shared formatting helpers. Pure functions, no React, no state. */

/** `HH:MM:SS` in local time; invalid timestamps render as `--:--:--`. */
export function formatClock(iso: string): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return '--:--:--'
  return date.toLocaleTimeString('zh-CN', { hour12: false })
}

/** Compact relative time for the thread list (`3 分钟前`). */
export function formatRelative(iso: string, now = Date.now()): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return ''
  const deltaSec = Math.max(0, Math.round((now - date.getTime()) / 1000))
  if (deltaSec < 60) return '刚刚'
  if (deltaSec < 3600) return `${Math.floor(deltaSec / 60)} 分钟前`
  if (deltaSec < 86_400) return `${Math.floor(deltaSec / 3600)} 小时前`
  return `${Math.floor(deltaSec / 86_400)} 天前`
}

/** Tool arguments arrive as arbitrary JSON; show them readably, not raw-minified. */
export function formatToolArguments(value: unknown): string {
  if (value === undefined || value === null) return ''
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

/** Token counts are provider counters; only the presentation is formatted. */
export function formatTokens(value: number): string {
  if (!Number.isFinite(value)) return '0'
  if (value < 1000) return String(value)
  return `${(value / 1000).toFixed(1)}k`
}
