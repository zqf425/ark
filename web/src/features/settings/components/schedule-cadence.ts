/**
 * Cadence presentation: display text only, never scheduling arithmetic.
 *
 * `next_run_at` and the normalized cadence come from the runtime
 * (docs/modules/web.md: 客户端不做 next_run 计算), so everything here is a pure
 * mapping from a wire `TaskSchedule` to a sentence or a local short timestamp.
 * The translator is passed in, so this module stays free of React and of the
 * catalog itself.
 */
import type { MessageKey, MessageParams } from '../../../shared/i18n/messages'
import type { ScheduleKind, TaskSchedule } from '../../../shared/contracts'

type Translator = (key: MessageKey, params?: MessageParams) => string

/** The five cadences, in the order the form offers them. */
export const SCHEDULE_KINDS: readonly ScheduleKind[] = ['manual', 'at', 'interval', 'daily', 'weekly', 'monthly']

/** `0` = Sunday, matching `TaskSchedule.day_of_week`. */
export const WEEKDAYS: readonly number[] = [0, 1, 2, 3, 4, 5, 6]

/** `1..31`, matching `TaskSchedule.day_of_month`. */
export const MONTH_DAYS: readonly number[] = Array.from({ length: 31 }, (_, index) => index + 1)

/** Wire autonomy ids; the labels come from the existing `session.runtime.*` keys. */
export const SCHEDULE_AUTONOMY_VALUES: readonly string[] = ['strict', 'workspace_write', 'unattended']

/** Label key for one cadence option (also used by the list's kind chip). */
export const SCHEDULE_KIND_LABEL_KEYS: Record<ScheduleKind, MessageKey> = {
  manual: 'settings.schedules.kind.manual',
  at: 'settings.schedules.kind.at',
  interval: 'settings.schedules.kind.interval',
  daily: 'settings.schedules.kind.daily',
  weekly: 'settings.schedules.kind.weekly',
  monthly: 'settings.schedules.kind.monthly'
}

/** `0` = Sunday; used by the weekly picker and the summary sentence. */
export function weekdayLabelKey(day: number): MessageKey {
  const safe = day >= 0 && day <= 6 ? day : 0
  return `settings.schedules.weekday.${safe}` as MessageKey
}

/** `HH:mm` as stored; `--:--` when the runtime sent neither (never guessed here). */
function clock(raw: string): string {
  const trimmed = raw.trim()
  return trimmed === '' ? '--:--' : trimmed
}

/** Sorted, deduplicated day list, so the summary and the picker agree. */
function sortedDays(days: readonly number[]): number[] {
  return [...new Set(days)].sort((left, right) => left - right)
}

/**
 * One sentence per cadence: 「每 30 分钟」/「每天 09:00」/「每周一、周三 20:00」.
 *
 * A weekly task with no weekday is described as the runtime will run it
 * (Monday), rather than as an empty set the user cannot interpret.
 */
export function describeSchedule(schedule: TaskSchedule, t: Translator): string {
  switch (schedule.kind) {
    case 'manual':
      return t('settings.schedules.cadence.manual')
    case 'at':
      return schedule.at_time.trim() === ''
        ? t('settings.schedules.cadence.atNone')
        : t('settings.schedules.cadence.at', { time: formatLocalStamp(schedule.at_time) })
    case 'interval':
      return t('settings.schedules.cadence.interval', { minutes: schedule.every_minutes })
    case 'daily':
      return t('settings.schedules.cadence.daily', { time: clock(schedule.time_of_day) })
    case 'weekly': {
      const days = sortedDays(schedule.day_of_week)
      const names = (days.length > 0 ? days : [1])
        .map((day) => t(weekdayLabelKey(day)))
        .join(t('common.listSeparator'))
      return t('settings.schedules.cadence.weekly', { days: names, time: clock(schedule.time_of_day) })
    }
    case 'monthly': {
      const days = sortedDays(schedule.day_of_month)
      const shown = days.length > 0 ? days : [1]
      return t('settings.schedules.cadence.monthly', {
        days: shown.join(t('common.listSeparator')),
        time: clock(schedule.time_of_day)
      })
    }
    default:
      return t('settings.schedules.cadence.manual')
  }
}

/** Short local date+time for `next_run_at`; the empty string means "not scheduled". */
export function formatLocalStamp(iso: string, locale: 'zh-CN' | 'en-US' = 'zh-CN'): string {
  const trimmed = iso.trim()
  if (trimmed === '') return ''
  const date = new Date(trimmed)
  if (Number.isNaN(date.getTime())) return trimmed
  return date.toLocaleString(locale, { dateStyle: 'short', timeStyle: 'short' })
}

/** Display form for `next_run_at`: an explicit 「不调度」 instead of a blank cell. */
export function describeNextRun(iso: string, t: Translator, locale: 'zh-CN' | 'en-US' = 'zh-CN'): string {
  const shown = formatLocalStamp(iso, locale)
  return shown === '' ? t('settings.schedules.nextRun.none') : shown
}
