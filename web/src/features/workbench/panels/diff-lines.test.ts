import { describe, expect, it } from 'vitest'
import { diffLineCounts, parseUnifiedDiff } from './diff-lines'

const PATCH = `diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@
 const a = 1
-const b = 2
+const b = 3
+const c = 4
 export { a, b }
`

describe('parseUnifiedDiff', () => {
  it('按 hunk 头推进行号并区分增删上下文行', () => {
    const lines = parseUnifiedDiff(PATCH)

    const hunk = lines.find((line) => line.type === 'meta' && line.text.startsWith('@@'))
    expect(hunk).toBeDefined()

    const context = lines.filter((line) => line.type === 'context')
    expect(context[0]).toEqual({ type: 'context', text: 'const a = 1', oldNumber: 1, newNumber: 1 })

    const removed = lines.filter((line) => line.type === 'remove')
    expect(removed).toEqual([{ type: 'remove', text: 'const b = 2', oldNumber: 2, newNumber: null }])

    const added = lines.filter((line) => line.type === 'add')
    expect(added).toEqual([
      { type: 'add', text: 'const b = 3', oldNumber: null, newNumber: 2 },
      { type: 'add', text: 'const c = 4', oldNumber: null, newNumber: 3 }
    ])

    // The trailing context line resumes numbering after the additions.
    const last = lines[lines.length - 1]
    expect(last).toEqual({ type: 'context', text: 'export { a, b }', oldNumber: 3, newNumber: 4 })
  })

  it('统计增删行数', () => {
    expect(diffLineCounts(parseUnifiedDiff(PATCH))).toEqual({ additions: 2, deletions: 1 })
  })

  it('对不可识别行降级为 meta 而不是丢弃', () => {
    const lines = parseUnifiedDiff('\\ No newline at end of file\n')
    expect(lines).toHaveLength(1)
    expect(lines[0]?.type).toBe('meta')
  })
})
