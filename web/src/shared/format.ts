/** Shared formatting helpers. Pure functions, no React, no state. */
import type { UiLocale } from './contracts/web-only'

/** `HH:MM:SS` in local time; invalid timestamps render as `--:--:--`. */
export function formatClock(iso: string, locale: UiLocale = 'zh-CN'): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return '--:--:--'
  return date.toLocaleTimeString(locale, { hour12: false })
}

/**
 * Compact relative time for the thread list (`3 分钟前` / `3m ago`).
 *
 * The wording comes from the caller's translator so this module stays free of
 * React and the i18n catalog: pass `t` and the key resolution happens upstream.
 */
export function formatRelative(
  iso: string,
  t: (key: 'canvas.relative.justNow' | 'canvas.relative.minutes' | 'canvas.relative.hours' | 'canvas.relative.days', params?: { count: number }) => string,
  now = Date.now()
): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return ''
  const deltaSec = Math.max(0, Math.round((now - date.getTime()) / 1000))
  if (deltaSec < 60) return t('canvas.relative.justNow')
  if (deltaSec < 3600) return t('canvas.relative.minutes', { count: Math.floor(deltaSec / 60) })
  if (deltaSec < 86_400) return t('canvas.relative.hours', { count: Math.floor(deltaSec / 3600) })
  return t('canvas.relative.days', { count: Math.floor(deltaSec / 86_400) })
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

/**
 * `cost_usd` is USD by contract, but its rendering follows the display locale:
 * the currency symbol is never hardcoded in components.
 */
export function formatUsd(value: number, locale: UiLocale): string {
  if (!Number.isFinite(value)) return new Intl.NumberFormat(locale, { style: 'currency', currency: 'USD' }).format(0)
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 2,
    maximumFractionDigits: 4
  }).format(value)
}
