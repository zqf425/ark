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
    // The security property is structural: parsing the output must yield no
    // element from the source. (A string scan would false-positive on the
    // quoted `data-copy-source` attribute, where a literal `<` is legal.)
    const host = document.createElement('div')
    host.innerHTML = html
    expect(host.querySelector('b')).toBeNull()
    expect(host.querySelector('pre code')?.textContent).toContain('<b>&raw')
  })

  it('代码块带语言标签与复制按钮,复制源是原始代码', () => {
    const html = renderMarkdown('```ts\nconst a = 1\n```\n')
    expect(html).toContain('ark-md-code__bar')
    expect(html).toContain('ark-md-code__lang">ts')
    expect(html).toContain('data-md-copy')
    expect(html).toContain('data-copy-source="const a = 1"')
  })

  it('无语言标注的代码块语言标签回落为 text', () => {
    const html = renderMarkdown('```\nplain\n```\n')
    expect(html).toContain('ark-md-code__lang">text')
  })
})
