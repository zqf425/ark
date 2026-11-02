// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { renderMarkdown } from './markdown-render'

describe('renderMarkdown', () => {
  it('渲染标题、列表与代码块', () => {
    const html = renderMarkdown('# Title\n\n- item\n\n```ts\nconst a = 1\n```\n')
    expect(html).toContain('<h1>Title</h1>')
    expect(html).toContain('<li>item</li>')
    expect(html).toContain('<pre><code class="hljs"')
  })

  it('剥离脚本与事件属性（模型输出不可信）', () => {
    const html = renderMarkdown('hello <script>alert(1)</script><img src=x onerror="alert(1)">')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('onerror')
  })

  it('保留高亮 span 的 class', () => {
    const html = renderMarkdown('```js\nconst a = 1\n```\n')
    expect(html).toContain('hljs-keyword')
  })

  it('未知名语言的代码块仍被转义展示', () => {
    const html = renderMarkdown('```weirdlang\n<b>&raw\n```\n')
    // highlightAuto may wrap the text in spans; the security property is that
    // the source never appears as raw markup.
    expect(html).toContain('&lt;')
    expect(html).not.toContain('<b>')
  })
})
