import { describe, expect, it } from 'vitest'
import { MESSAGES, translate, type MessageKey } from './messages'

/**
 * Every key in the catalog, not a hand-picked subset: the point of this suite is
 * to catch a feature that adds a key to one language only, or copies the Chinese
 * sentence into the English column and calls it translated.
 */
const KEYS = Object.keys(MESSAGES) as MessageKey[]

/** CJK ideographs and full-width punctuation that must not leak into `en-US`. */
const CJK = /[\u3000-\u303f\u4e00-\u9fff\uff00-\uffef]/

/** `{name}` slots, so a placeholder can be compared across both languages. */
const PLACEHOLDERS = /\{(\w+)\}/g

function placeholders(value: string): string[] {
  return [...value.matchAll(PLACEHOLDERS)]
    .map((match) => match[1] ?? '')
    .sort()
}

describe('i18n catalog completeness', () => {
  it('每个键都有中英文两列，且都不是空字符串', () => {
    expect(KEYS.length).toBeGreaterThan(0)
    for (const key of KEYS) {
      const entry = MESSAGES[key]
      expect(entry, key).toHaveLength(2)
      expect(entry[0].trim(), `${key} (zh-CN)`).not.toBe('')
      expect(entry[1].trim(), `${key} (en-US)`).not.toBe('')
    }
  })

  it('英文列不含中文，避免切换语言后仍显示中文', () => {
    for (const key of KEYS) {
      expect(MESSAGES[key][1], key).not.toMatch(CJK)
    }
  })

  it('英文列不是中文的逐字复制，避免假翻译', () => {
    for (const key of KEYS) {
      const [zh, en] = MESSAGES[key]
      // Identical values are legitimate only for identifiers and placeholders
      // (`id`, `model`, `Code`, `openai_chat`, `sk-…`), never for prose: a
      // Chinese sentence copied into the English column would be untranslated.
      if (zh === en) expect(zh, `${key} looks untranslated`).not.toMatch(CJK)
    }
  })

  it('同一键的两种语言使用相同的占位符集合', () => {
    for (const key of KEYS) {
      const [zh, en] = MESSAGES[key]
      expect(placeholders(en), key).toEqual(placeholders(zh))
    }
  })

  it('translate 对每个键都返回非空结果', () => {
    for (const key of KEYS) {
      expect(translate('zh-CN', key), `zh-CN ${key}`).not.toBe('')
      expect(translate('en-US', key), `en-US ${key}`).not.toBe('')
    }
  })
})

describe('i18n catalog coverage of the swept surfaces', () => {
  it('覆盖简报点名的每一类界面文案', () => {
    const prefixes = [
      'common.',
      'status.',
      'api.',
      'thread.list.',
      'thread.row.',
      'canvas.',
      'session.',
      'settings.',
      'workbench.',
      'error.hint.'
    ]
    for (const prefix of prefixes) {
      expect(KEYS.some((key) => key.startsWith(prefix)), prefix).toBe(true)
    }
  })

  it('错误分类的每个 code 都有本地化提示', () => {
    const codes = [
      'validation_error',
      'not_found',
      'unauthorized',
      'conflict',
      'model_request_failed',
      'tool_execution_failed',
      'runtime_unavailable',
      'store_failure',
      'not_implemented',
      'internal_error'
    ]
    for (const code of codes) {
      expect(KEYS).toContain(`error.hint.${code}` as MessageKey)
      expect(translate('en-US', `error.hint.${code}` as MessageKey), code).not.toMatch(CJK)
    }
  })
})
