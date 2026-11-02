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

  it('hunk 内以 ---/+++ 开头的删除与新增行不被误判为文件头', () => {
    const patch = `--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,2 @@
 ctx
- --- a/fake-header
+ +++ b/fake-header
`
    const lines = parseUnifiedDiff(patch)
    const removes = lines.filter((line) => line.type === 'remove')
    expect(removes.map((line) => line.text)).toEqual([' --- a/fake-header'])
    expect(removes[0]?.oldNumber).toBe(2)
    const adds = lines.filter((line) => line.type === 'add')
    expect(adds.map((line) => line.text)).toEqual([' +++ b/fake-header'])
    expect(adds[0]?.newNumber).toBe(2)
  })

  it('多 hunk 行号各自从 @@ 头续接', () => {
    const patch = `--- a/f
+++ b/f
@@ -1,2 +1,2 @@
 a
-b
+c
 d
@@ -10,2 +10,2 @@
 e
-f
+g
 h
`
    const lines = parseUnifiedDiff(patch)
    const numbered = lines.filter((line) => line.type !== 'meta')
    expect(numbered).toEqual([
      { type: 'context', text: 'a', oldNumber: 1, newNumber: 1 },
      { type: 'remove', text: 'b', oldNumber: 2, newNumber: null },
      { type: 'add', text: 'c', oldNumber: null, newNumber: 2 },
      { type: 'context', text: 'd', oldNumber: 3, newNumber: 3 },
      { type: 'context', text: 'e', oldNumber: 10, newNumber: 10 },
      { type: 'remove', text: 'f', oldNumber: 11, newNumber: null },
      { type: 'add', text: 'g', oldNumber: null, newNumber: 11 },
      { type: 'context', text: 'h', oldNumber: 12, newNumber: 12 }
    ])
  })

  it('不可解析的 @@ 行降级为 meta，且不武装 hunk 状态', () => {
    // Combined-diff `@@@` headers are outside the renderer's contract; they
    // must not crash parsing nor arm hunk numbering.
    const lines = parseUnifiedDiff('@@@ -1,2,3 +1,2,3 @@\nplain line\n')
    expect(lines).toHaveLength(2)
    expect(lines.every((line) => line.type === 'meta')).toBe(true)
    expect(lines[1]?.oldNumber).toBeNull()
  })

  it('diff --git 重新武装文件头识别（下一个文件段）', () => {
    const patch = `diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
diff --git a/y b/y
--- a/y
+++ b/y
@@ -1 +1 @@
-c
+d
`
    const lines = parseUnifiedDiff(patch)
    const headers = lines.filter((line) => line.text.startsWith('--- a/'))
    expect(headers).toHaveLength(2)
    expect(headers.every((line) => line.type === 'meta')).toBe(true)
    // Second segment's content still gets its own numbering.
    const secondHunkAdd = lines.filter((line) => line.type === 'add').at(-1)
    expect(secondHunkAdd).toEqual({ type: 'add', text: 'd', oldNumber: null, newNumber: 1 })
  })
})
