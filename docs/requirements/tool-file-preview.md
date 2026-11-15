# 工具卡文件预览入口(写文件类工具点击查看)

> 状态:**已批准计划**(2026-11-05)。
> 让会话流里的 `write`/`edit` 类工具卡成为文件预览入口:卡片头部显示目标文件名,
> 点击即在右侧检查器 File 面板打开该文件(`.md` 自动获得富文本渲染),
> 对齐主流 agent 画布"写文件出现可点击文件卡"的心智。

## 背景与目标

现状缺口(见 `workspace-file-preview.md` 的已实现部分):

1. 数据与渲染面已就绪:`GET /v1/workspace/file?path=` 可读工作区任意文件,
   FilePanel 对 `.md`/`.markdown` 用共享 `<Markdown>` 管线富文本渲染,其余走代码高亮;
2. 唯一的 `selectFile` 入口在 Changes 面板的 diff 文件行,会话流里 `write`/`edit`
   工具调用只渲染成可折叠 ToolCard(文本摘要 + 参数/输出 `<pre>`),
   用户从会话里看不到"刚写了哪个文件、点开看看"的直接反馈。

本需求把"写文件"从一条纯文本日志升级为可交互文件芯片:
文件名在卡片头部可读、可点击,点击 = `selectFile(path)`(切到 File 面板并选中)。

非目标:不新增后端接口;不显示逐文件 `+N -M` 统计(需要按路径拉取 patch,
ToolCard 是长回合高频行,不值得为此加请求);不改变 ToolCard 的折叠展开行为
(文件芯片与摘要、状态、耗时并存);不处理 `bash` 等间接写文件的场景
(无结构化 path 参数,不做启发式猜测)。

## 设计

### 1. 路径提取(唯一宿主:`code-session/tool-file.ts`)

纯函数,输入 `ToolPayload`(生成契约),输出文件芯片数据或 null:

```text
toolTargetPath(tool: ToolPayload): string | null
```

- 仅当 `tool.tool` 是 `write`/`edit` 且 `arguments` 是对象、`path` 是非空字符串时返回路径;
- 其它工具(含 `read`、`bash`)一律 null——入口只承诺"结构化写入过的文件",
  不做工具名启发式之外的猜测;
- 与 `formatToolArguments` 同层级(纯函数、无 React、无 i18n),可独立单测。

### 2. ToolCard 渲染(组件内接线)

`ToolCard.tsx` 在既有折叠卡头部追加文件芯片:

- `toolTargetPath` 非空时,在工具名与 digest 之间渲染一枚文件芯片
  (文件基名 + 收敛后的目录提示,mono 小字,hover 强调);
- 芯片是独立 `<button>`,`onClick` 调用 workbench slice 的 `selectFile(path)`,
  `e.stopPropagation()` 阻断卡片折叠开关——点击文件名不再展开参数/输出原文;
- FilePanel 加载该路径即自动获得既有渲染分派(`.md` 富文本、其余高亮、binary/truncated 空态);
- 无路径的工具卡维持现状,零行为变化。

### 3. 状态接线

`selectFile` 来自 `useCodeSessionStore`(workbench-slice,既有 action):
设置 `selectedFilePath` 并把检查器切到 `file` Tab。检查器收起时**不自动展开**
(与 Changes 面板行点击同语义:能点到卡片说明用户正在看画布,是否展开检查器由用户决定;
但 Tab 切换保证展开后看到的是 File 面板)。

## 验收

- `tool-file.test.ts`:write/edit 提取路径、非对象参数、无 path、其它工具名返回 null。
- ToolCard 组件测试:有路径的 write 卡渲染文件芯片、点击调用 `selectFile` 且不切换卡片折叠态;
  无路径卡片不渲染芯片。
- i18n:芯片 title 提示走消息键(中英双语,`catalog-completeness` 守卫自动覆盖)。
- web 三件套(tsc / vitest / check:rules)全绿;UI 变更做真实浏览器验证
  (构建 ark 打开会话,触发 write 后点芯片,File 面板渲染)。
- `docs/modules/web.md` 同步:工具卡条目补充文件芯片行为。
