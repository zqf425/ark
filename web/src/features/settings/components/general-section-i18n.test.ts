import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/** Source files the brief covers; comments may stay Chinese, user-facing text may not. */
const GENERAL_SECTION = 'settings/components/GeneralSection.tsx'
const SECTION_SHELL = 'settings/components/SectionShell.tsx'
const RUNTIME_STATUS_BAR = 'code-session/components/RuntimeStatusBar.tsx'
const COVERED = [GENERAL_SECTION, SECTION_SHELL, RUNTIME_STATUS_BAR]

const CJK = /[\u3000-\u303f\u4e00-\u9fff\uff00-\uffef]/

function readComponent(relative: string): string {
  return readFileSync(fileURLToPath(new URL(`../../${relative}`, import.meta.url)), 'utf8')
}

/** Drop block and line comments so only executable strings remain. */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .filter((line) => !line.trimStart().startsWith('//'))
    .join('\n')
}

describe('general section and status bar copy', () => {
  it('覆盖的两个组件不再硬编码中文用户文案', () => {
    for (const relative of COVERED) {
      const code = stripComments(readComponent(relative))
      expect(code, relative).not.toMatch(CJK)
    }
  })

  it('通用区块与状态栏的文案都走消息键', () => {
    const general = stripComments(readComponent(GENERAL_SECTION))
    expect(general).toContain("t('settings.general.title')")
    expect(general).toContain("t('settings.general.workspace.placeholder')")
    expect(general).toContain("t('settings.general.logLevel.label')")

    const statusBar = stripComments(readComponent(RUNTIME_STATUS_BAR))
    expect(statusBar).toContain("'status.online.unknown'")
    expect(statusBar).toContain("'status.stream.closed'")
    expect(statusBar).toContain("t('status.refresh')")
  })
})
