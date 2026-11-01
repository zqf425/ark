/**
 * Cadence summaries are the only cadence copy the user reads, and they are
 * built from runtime-normalized values. These cases pin the wording of all five
 * cadences (plus the two fallbacks the runtime applies) so a change in the
 * sentence pattern is a test failure rather than a silently wrong label.
 */
import { describe, expect, it } from 'vitest'
import type { TaskSchedule } from '../../../shared/contracts'
import { translate, type MessageKey, type MessageParams } from '../../../shared/i18n/messages'
import { describeNextRun, describeSchedule, formatLocalStamp, WEEKDAYS, weekdayLabelKey } from './schedule-cadence'

/** The catalog's own translator, so the assertions read real user copy. */
function t(locale: 'zh-CN' | 'en-US') {
  return (key: MessageKey, params?: MessageParams): string => translate(locale, key, params)
}

function schedule(patch: Partial<TaskSchedule>): TaskSchedule {
  return {
    kind: 'manual',
    every_minutes: 0,
    time_of_day: '',
    at_time: '',
    day_of_week: [],
    day_of_month: [],
    ...patch
  }
}

describe('schedule cadence summaries', () => {
  it('describes each of the five cadences in both languages', () => {
    expect(describeSchedule(schedule({ kind: 'manual' }), t('zh-CN'))).toBe('仅手动运行')
    expect(describeSchedule(schedule({ kind: 'interval', every_minutes: 30 }), t('zh-CN'))).toBe('每 30 分钟')
    expect(describeSchedule(schedule({ kind: 'interval', every_minutes: 30 }), t('en-US'))).toBe('Every 30 minutes')
    expect(describeSchedule(schedule({ kind: 'daily', time_of_day: '09:00' }), t('zh-CN'))).toBe('每天 09:00')
    expect(describeSchedule(schedule({ kind: 'daily', time_of_day: '09:00' }), t('en-US'))).toBe('Daily at 09:00')
  })

  it('joins weekly days in sorted order and ignores duplicates', () => {
    const weekly = schedule({ kind: 'weekly', time_of_day: '20:00', day_of_week: [3, 1, 3] })
    expect(describeSchedule(weekly, t('zh-CN'))).toBe('每周一、周三 20:00')
    expect(describeSchedule(weekly, t('en-US'))).toBe('Every Mon, Wed at 20:00')
  })

  it('describes a monthly task by its days and time', () => {
    const monthly = schedule({ kind: 'monthly', time_of_day: '08:30', day_of_month: [15, 1] })
    expect(describeSchedule(monthly, t('zh-CN'))).toBe('每月 1、15 日 08:30')
  })

  it('shows the runtime fallback instead of an empty weekly or monthly set', () => {
    // The runtime stores Monday / the 1st when the user clears the picker.
    expect(describeSchedule(schedule({ kind: 'weekly', time_of_day: '07:00' }), t('zh-CN'))).toBe('每周一 07:00')
    expect(describeSchedule(schedule({ kind: 'monthly', time_of_day: '07:00' }), t('zh-CN'))).toBe('每月 1 日 07:00')
  })

  it('states a missing one-shot instant instead of inventing one', () => {
    expect(describeSchedule(schedule({ kind: 'at' }), t('zh-CN'))).toBe('一次性（未设置时刻）')
  })

  it('labels every weekday, including Sunday as 0', () => {
    expect(WEEKDAYS).toEqual([0, 1, 2, 3, 4, 5, 6])
    expect(translate('zh-CN', weekdayLabelKey(0))).toBe('周日')
    expect(translate('en-US', weekdayLabelKey(6))).toBe('Sat')
  })

  it('renders next_run_at as local time and marks the empty string as not scheduled', () => {
    const iso = '2026-11-03T01:00:00Z'
    // Local rendering depends on the machine's zone, so assert the shape only.
    expect(formatLocalStamp(iso, 'en-US')).toMatch(/\d/)
    expect(formatLocalStamp(iso)).not.toBe('')
    expect(describeNextRun('', t('zh-CN'))).toBe('不调度')
    expect(describeNextRun('', t('en-US'))).toBe('Not scheduled')
  })

  it('never hides an unparseable timestamp', () => {
    expect(formatLocalStamp('not-a-date')).toBe('not-a-date')
    expect(describeNextRun('not-a-date', t('zh-CN'))).toBe('not-a-date')
  })
})
