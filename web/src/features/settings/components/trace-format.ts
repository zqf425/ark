/**
 * Trace-section formatting helpers. Pure functions, no React, no fetch.
 *
 * Duration text reuses the shared spelled-out clock (`shared/format.ts`'s
 * `formatDurationWords`) so "逐回合耗时" reads exactly like the canvas header
 * and stats bar; the unit words come from the catalog via `translateNow`,
 * keeping this module free of hardcoded prose. The locale parameter stays in
 * the signature for future readouts that need it (locale-tagged numbers).
 */
import type { UiLocale } from '../../../shared/contracts/web-only'
import { translateNow } from '../../../shared/i18n/current'
import { formatDurationWords, type DurationUnitWords } from '../../code-session/components/session-stats'

/** Localized spelled-out span (`2分3秒`) for a millisecond measure. */
export function formatDurationWordsMs(ms: number, _locale: UiLocale): string {
  const units: DurationUnitWords = {
    hours: (count) => translateNow('canvas.duration.hours', { count }),
    minutes: (count) => translateNow('canvas.duration.minutes', { count }),
    seconds: (count) => translateNow('canvas.duration.seconds', { count })
  }
  return formatDurationWords(ms, units)
}
