import { describe, expect, it } from 'vitest'
import { MESSAGES, translate, type MessageKey } from './messages'

const GENERAL_KEYS = Object.keys(MESSAGES).filter((key) => key.startsWith('settings.general.')) as MessageKey[]
const STATUS_KEYS = Object.keys(MESSAGES).filter((key) => key.startsWith('status.')) as MessageKey[]

/** CJK ideographs and full-width punctuation that must not leak into `en-US`. */
const CJK = /[\u3000-\u303f\u4e00-\u9fff\uff00-\uffef]/

describe('general section and status bar i18n keys', () => {
  it('中英文资源都提供通用区块与状态栏的消息键', () => {
    expect(GENERAL_KEYS.length).toBeGreaterThan(0)
    expect(STATUS_KEYS.length).toBeGreaterThan(0)
    for (const key of [...GENERAL_KEYS, ...STATUS_KEYS]) {
      expect(translate('zh-CN', key), key).not.toBe('')
      expect(translate('en-US', key), key).not.toBe('')
    }
  })

  it('英文资源不含未翻译的中文，避免切换语言后仍显示中文', () => {
    for (const key of [...GENERAL_KEYS, ...STATUS_KEYS]) {
      expect(translate('en-US', key), key).not.toMatch(CJK)
    }
  })

  it('覆盖简报点名的区块标题、工作空间 placeholder 与状态栏文案', () => {
    expect(translate('zh-CN', 'settings.general.title')).toBe('通用')
    expect(translate('en-US', 'settings.general.title')).toBe('General')
    expect(translate('en-US', 'settings.general.workspace.placeholder')).toBe('D:\\code\\project or /home/me/project')
    expect(translate('zh-CN', 'settings.general.workspace.placeholder')).toContain('或')
    expect(translate('en-US', 'status.online.online')).toBe('Online')
    expect(translate('en-US', 'status.stream.closed')).toBe('Event stream closed')
    expect(translate('en-US', 'status.refresh')).toBe('Recheck runtime')
    expect(translate('zh-CN', 'status.refresh')).toBe('重新检查运行时')
  })

  it('log level 的说明后缀也本地化', () => {
    expect(translate('zh-CN', 'settings.general.logLevel.trace')).toContain('最详细')
    expect(translate('en-US', 'settings.general.logLevel.trace')).toContain('verbose')
    expect(translate('en-US', 'settings.general.logLevel.off')).toContain('disabled')
  })
})

describe('translate placeholders', () => {
  it('替换 {name} 占位符并按语言切换句子', () => {
    expect(translate('zh-CN', 'settings.general.defaultModel.switched', { providerId: 'deepseek' })).toBe('默认模型已切换为 deepseek')
    expect(translate('en-US', 'settings.general.defaultModel.switched', { providerId: 'deepseek' })).toBe('Default model switched to deepseek')
    expect(translate('en-US', 'status.refreshFailed', { error: 'HTTP 401' })).toBe('Recheck runtime. Last failure: HTTP 401')
    expect(translate('zh-CN', 'settings.general.changed', { keys: 'log_level' })).toBe('已变更：log_level')
  })

  it('缺少参数时保留原占位符，而不是打印 undefined', () => {
    expect(translate('en-US', 'status.refreshFailed')).toBe('Recheck runtime. Last failure: {error}')
    expect(translate('en-US', 'status.refreshFailed', { other: 'x' })).toBe('Recheck runtime. Last failure: {error}')
  })

  it('无参数的键保持原样', () => {
    expect(translate('en-US', 'common.retry')).toBe('Retry')
    expect(translate('zh-CN', 'common.retry')).toBe('重试')
  })
})
