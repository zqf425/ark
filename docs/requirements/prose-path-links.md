# 助手正文文件路径可点击(点击在 File 面板打开)

> 状态:**已实现**(2026-11-05;同日修订:basename 三级解析,正文惯用纯文件名也能命中)。
> 助手回复正文里引用的文件路径渲染为可点击芯片,点击与工具卡文件芯片同语义:
> `selectFile(path)` 在右侧检查器 File 面板打开(`.md` 自动富文本渲染)。

## 背景与目标

工具卡文件芯片(`tool-file-preview.md`)已上线,但用户在**正文**里读到的路径
(`.tmp/markdown-view-2.md` 这类行内代码)仍是纯文本——主流 agent 画布的正文
文件引用是可交互的,正文才是用户阅读的主场,路径却是死的,心智断裂。

目标:助手正文(`AssistantBubble` 中间叙述 + `FinalReplyCard` 最终回复)里
出现的文件路径渲染为可点击芯片,点击复用 `selectFile`。
用户消息正文(已有 mention 芯片管线)与本需求无关,不动。

## 设计

### 1. 识别范围(唯一宿主:`shared/ui/markdown-render.ts` 后处理)

在 `renderMarkdown` 的 DOMPurify 之前对解析结果做一次受限的 DOM 后处理,
**只把行内 `<code>` 片段升级为文件芯片**:

- 片段文本是"路径形":含 `/` 或 `\`,不以 `/` 结尾,不含空格、反引号、
  协议前缀(`http://` 等)、`~` 开头的家目录路径(工作区边界语义不允许);
- **白名单约束(误报控制)**:只有路径存在于**本回合写过的文件集合**
  (`threadPaths: readonly string[]`)中才渲染为芯片——该集合由调用方从
  本线程所有 `write`/`edit` 工具参数的 `path` 投影而来(复用
  `code-session/tool-file.ts` 的提取),渲染层不做文件系统猜测;
  集合为空时整个后处理零开销直通;
- 代码块(`pre code`)不参与:围栏代码是程序文本,路径只是巧合;
- 路匹匹配三级解析:精确 → `/`→`\` 归一 → **唯一 basename**。实测模型正文
  惯用纯文件名(`markdown-demo.md`)而写入参数是绝对路径,逐字一致才可信的
  初版策略把最常见的引用形态挡在了门外;basename 只有在白名单内唯一时才解析
  (同名多文件不升级,点击无法指认);解析结果一律还原为白名单原路径交给
  `selectFile`,正文写法不参与打开语义。无分隔符的片段须含 `.` 才视为文件名
  (纯单词/命令不误伤)。

### 2. 安全边界

- 芯片 DOM 是 renderer 自产的:`<button type="button" class="ark-md-filechip"
  data-md-file="...">`,属性值经既有 `escapeAttr`;不引入任何模型可控的
  `href`/事件属性,DOMPurify 默认白名单原样放行(button/无 href 无风险);
- 点击不在 HTML 里绑定:`Markdown` 组件的既有委托 `onClick` 扩展一个分支,
  `closest('[data-md-file]')` 命中时回调 `onFileChip(path)` 后
  `stopPropagation`(阻断祖先点击语义);
- `shared/ui` 不反依 feature:`Markdown` 新增可选 prop `onFileChip?: (path:
  string) => void`,`selectFile` 的接线留在 `code-session` 组件侧
  (`MessageBubble`/`TurnBlock`),依赖方向不变。

### 3. 接线

- `turn-sections.ts` 已按回合分组;新增纯函数
  `threadWritePaths(items): string[]`(宿主 `code-session/tool-file.ts`,
  复用 `toolTargetPath`),从回合 items 投影去重路径集合;
- `AssistantBubble`/`FinalReplyCard` 接收 `threadId` 所在流的 items(或由
  `TurnBlock` 传 `writtenPaths` prop),传给 `Markdown`;
- 点击即 `selectFile(path)`——检查器收起时自动展开(上一需求已确立语义)。

## 验收

- `markdown-render.test.ts`:路径在白名单内→芯片;不在→原样 `<code>`;
  集合为空→零变更;`pre code` 不升级;协议前缀/家目录/空格路径不升级;
  输出仍过 DOMPurify。
- `tool-file.test.ts`:`threadWritePaths` 去重、顺序稳定。
- `Markdown.test`:委托点击命中 `data-md-file` 时回调携带路径。
- web 三件套全绿;真实浏览器验证(点击正文芯片 → File 面板渲染)。
- `docs/modules/web.md` 与本需求同步。
