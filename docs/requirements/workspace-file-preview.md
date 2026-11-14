# 工作区文件预览(含 Markdown 渲染)

> 状态:**已实现**(2026-11-05,离线可测部分;真实浏览器交互验收由发布验收承接)。
> 让 workbench 的 File 面板成为真正的文件预览器:任意工作区文件可读(不再限于本线程
> 曾用 `read` 读过的),Markdown 文件按富文本渲染,其余保持代码高亮。

## 背景与目标

现状两个缺口:

1. `GET /v1/threads/{id}/files` 是**线程事件投影**(`workbench::file_preview`):只回放
   本线程曾用 `read` 工具读过的文件,没读过的 404——它回答"这个线程读过什么",不是
   "这个文件是什么";
2. FilePanel 对所有扩展名统一走代码高亮,`.md` 显示带语法着色的原文而非渲染结果。

本需求新增**工作区真实文件预览**:以 active workspace root 为唯一授权边界,只读取该根
之下的文件;Markdown 渲染复用既有 `<Markdown>` 管线。

非目标:目录树浏览/文件列表 API;写路径(只读);`.md` 之外的渲染格式(HTML/图片);
工作区之外的任何路径(包括 `~` 展开后的家目录)。

## 设计

### 1. 后端读取(唯一宿主:`ark-workspace::WorkspaceInspector::read_file`)

`WorkspaceInspector` 已持有路径治理语义(validate/normalize),新增:

```text
fn read_file(&self, root: &Path, raw: &str) -> Result<FilePreview>
```

- **containment**:入参先经与 `validate` 相同的 normalize(`~` 不展开——预览以 root 为界,
  家目录不是工作区),绝对/相对统一后做 `canonicalize`,再断言 canonical 结果仍在
  canonical root 之下;symlink 指向根外即拒绝(与 `ark-tool` 的
  `ensure_real_path_inside` 同语义,本 crate 自持一份最小实现,不反向依赖 ark-tool)。
- **上限**:UTF-8 文本最多 **256 KiB**(与 `DEFAULT_LIMITS.max_bytes` 同量级),超出部分
  截断并置 `truncated: true`;非 UTF-8 文件读取为 lossy 文本并置 `binary: true`
  (前端只提示不渲染);单文件超过 **8 MiB** 直接结构化拒绝(与 read 工具的
  `MAX_LEGACY_DECODE_BYTES` 一致)。
- **无套件依赖**:纯 `std::fs`,无 git、无 tokio;handler 侧由调用方包
  `spawn_blocking`(与 `workspace/pick` 同一先例)。

### 2. 契约(ark-contract,wire + TS 生成)

```text
WorkspaceFilePreview {
  path: string            # 展示用相对路径(规范化后的入参原样返回)
  content: string         # UTF-8 文本(lossy);超限截断
  truncated: bool
  binary: bool            # lossy 解码产生了替换符
  size_bytes: u64
}
```

新增 `GET /v1/workspace/file?path=...`(鉴权路由组,与 workspace 其余读面同层)。
既有线程投影端点 `/v1/threads/{id}/files` 不动(它回答的是"线程读过什么",语义不同)。

### 3. 端口与接线

`WorkspaceService` 端口新增 `read_file(root, path) -> Result<WorkspaceFilePreview>`;
`WorkspaceAdapter` 转发 inspector 并以 `self.roots.lease().canonical_root` 为 root——
**回合外读取也使用同一 active root**,与 diff 视图一致。handler 在 `spawn_blocking`
中调用,遇 `WorkspaceRoots` 切换竞态时返回当前 lease 的结果,不做重试(预览是快照语义,
前端已有 generation 头丢弃旧响应的先例)。

### 4. 前端分派(FilePanel)

- 数据源从 `GET /v1/threads/{id}/files?path=` 切到 `GET /v1/workspace/file?path=`;
- 扩展名分派:`.md`/`.markdown` 用 `<Markdown>` 富文本渲染,其余走既有
  `highlightSource` 代码视图;`binary: true` 显示"二进制文件不预览"空态;
  `truncated: true` 显示截断提示;
- Changes 面板的既有 `selectFile` 行为不变(选中即切到 File 面板),因此 diff 列表里的
  `.md` 文件自动获得渲染视图。

## 验收

- `ark-workspace` 单测:根内相对/绝对读取、`..` 逃逸拒绝、symlink 指向根外拒绝、
  256 KiB 截断、非 UTF-8 lossy+binary 标记、8 MiB 拒绝、根外绝对路径拒绝。
- 契约导出后 `web/src/generated` 无漂移。
- 前端:`file-panel-dispatch` 单测(扩展名→渲染模式分派、binary/truncated 文案)。
- fmt / clippy -D warnings / workspace 测试 / web 三件套全绿。
