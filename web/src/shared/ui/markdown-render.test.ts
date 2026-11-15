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

  it('中文紧邻的 ** 强调不再残留字面星号（CJK flanking 修复）', () => {
    const html = renderMarkdown('这个技能是**在运行时上新建会话**的手册。')
    expect(html).toContain('<strong>在运行时上新建会话</strong>')
    expect(html).not.toContain('**')
  })

  it('拉丁文本的强调行为保持不变', () => {
    expect(renderMarkdown('plain **bold** text')).toContain('<strong>bold</strong>')
  })
})

describe('prose path chips (requirements/prose-path-links.md)', () => {
  const writable = ['.tmp/a.md', 'docs/guide.md']

  it('升级白名单内的行内 code 为文件芯片', () => {
    const html = renderMarkdown('写入 `docs/guide.md` 完成。', writable)
    expect(html).toContain('ark-md-filechip')
    expect(html).toContain('data-md-file="docs/guide.md"')
    expect(html).not.toContain('<code>docs/guide.md</code>')
  })

  it('白名单外的路径保持行内 code 原样', () => {
    const html = renderMarkdown('参考 `src/other.ts` 与 `docs/guide.md`。', writable)
    expect(html).toContain('<code>src/other.ts</code>')
    expect(html).toContain('ark-md-filechip')
  })

  it('白名单为空时零变更直通', () => {
    const html = renderMarkdown('写入 `docs/guide.md` 完成。')
    expect(html).not.toContain('ark-md-filechip')
    expect(html).toContain('<code>docs/guide.md</code>')
  })

  it('代码块内的路径不升级(围栏是程序文本)', () => {
    const html = renderMarkdown('```\ncat docs/guide.md\n```\n', writable)
    expect(html).not.toContain('ark-md-filechip')
  })

  it('协议前缀/家目录/空格路径不升级', () => {
    const html = renderMarkdown('`http://x/a.md` `~/a.md` `a b/c.md`', ['.tmp/a.md'])
    expect(html).not.toContain('ark-md-filechip')
  })

  it('反斜杠路径按归一匹配', () => {
    const html = renderMarkdown('写入 `docs\\guide.md`。', ['docs/guide.md'])
    expect(html).toContain('ark-md-filechip')
  })

  it('芯片是 renderer 自产 button,模型无法注入 href 或事件属性', () => {
    const html = renderMarkdown('写入 `docs/guide.md` <img src=x onerror="alert(1)">', ['docs/guide.md'])
    const host = document.createElement('div')
    host.innerHTML = html
    expect(host.querySelector('img')?.getAttribute('onerror')).toBeNull()
    const chip = host.querySelector('button.ark-md-filechip')
    expect(chip).not.toBeNull()
    expect(chip?.hasAttribute('href')).toBe(false)
  })
})
