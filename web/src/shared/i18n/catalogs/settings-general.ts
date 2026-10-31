/**
 * Settings -> Workspace (general) and the appearance/runtime subsections.
 *
 * Keys already referenced by `GeneralSection.tsx` are kept byte-identical: the
 * `general-section-i18n` and `general-status-messages` tests pin them.
 */
export const SETTINGS_GENERAL_MESSAGES = {
  'settings.general.title': ['通用', 'General'],
  'settings.general.desc': ['默认模型、工作空间与日志级别。', 'Default model, workspace, and log level.'],
  'settings.general.configLoadFailed': ['运行时配置读取失败', 'Failed to read the runtime configuration'],
  'settings.general.unknownError': ['未知错误', 'Unknown error'],
  'settings.general.saveFailed': ['保存失败', 'Save failed'],
  'settings.general.rejected': ['运行时拒绝了该配置', 'The runtime rejected this configuration'],
  'settings.general.changed': ['已变更：{keys}', 'Changed: {keys}'],
  'settings.general.changedSeparator': ['、', ', '],
  'settings.general.defaultModel.label': ['默认模型', 'Default model'],
  'settings.general.defaultModel.hint': ['选择即时生效：写入为 default provider。', 'Applies immediately: written as the default provider.'],
  'settings.general.defaultModel.noProviders': ['（暂无供应商）', '(no providers)'],
  'settings.general.defaultModel.unset': ['（未设置）', '(not set)'],
  'settings.general.defaultModel.active': ['真正在用', 'In use'],
  'settings.general.defaultModel.activeHint': [
    'active 表示当前实际发起请求的供应商；default 只是配置目标，运行中的会话可能要等到下一轮才切换。',
    'active is the provider actually serving requests; default is only the configured target, and a running thread may not switch until its next turn.'
  ],
  'settings.general.defaultModel.loadFailed': ['加载供应商失败', 'Failed to load providers'],
  'settings.general.defaultModel.switched': ['默认模型已切换为 {providerId}', 'Default model switched to {providerId}'],
  'settings.general.defaultModel.switchFailed': ['设置默认模型失败', 'Failed to set the default model'],
  'settings.general.workspace.label': ['默认工作空间', 'Default workspace'],
  'settings.general.workspace.hint': [
    '留空并保存 = 清除默认工作空间（提交空字符串）。',
    'Save an empty value to clear the default workspace (submits an empty string).'
  ],
  'settings.general.workspace.placeholder': ['D:\\code\\project 或 /home/me/project', 'D:\\code\\project or /home/me/project'],
  'settings.general.workspace.validate': ['校验', 'Validate'],
  'settings.general.workspace.validating': ['校验中…', 'Validating…'],
  'settings.general.workspace.save': ['保存', 'Save'],
  'settings.general.workspace.saving': ['保存中…', 'Saving…'],
  'settings.general.workspace.saved': ['默认工作空间已保存', 'Default workspace saved'],
  'settings.general.workspace.cleared': ['默认工作空间已清除', 'Default workspace cleared'],
  'settings.general.workspace.emptyPath': ['请先填写工作空间路径', 'Enter a workspace path first'],
  'settings.general.workspace.validateOk': ['路径校验通过', 'Path validated'],
  'settings.general.workspace.validateFailed': ['路径校验失败', 'Path validation failed'],
  'settings.general.workspace.statusFailed': ['工作空间状态读取失败', 'Failed to read the workspace status'],
  'settings.general.workspace.gitRepository': ['git 仓库', 'git repository'],
  'settings.general.workspace.exists': ['存在', 'exists'],
  'settings.general.workspace.missing': ['不存在', 'missing'],
  'settings.general.workspace.yes': ['是', 'yes'],
  'settings.general.workspace.no': ['否', 'no'],
  'settings.general.workspace.dirty': ['有未提交改动', 'uncommitted changes'],
  'settings.general.workspace.clean': ['干净', 'clean'],
  'settings.general.workspace.changeCount': ['变更数', 'Changed files'],
  'settings.general.workspace.reading': ['读取工作空间状态…', 'Reading the workspace status…'],
  'settings.general.workspace.probeHint': [
    '点「校验」可读取该路径的存在性、git 分支与改动状态。',
    'Select "Validate" to read this path\u2019s existence, git branch, and change state.'
  ],
  'settings.general.logLevel.label': ['日志级别', 'Log level'],
  'settings.general.logLevel.hint': ['保存后立即生效（runtime 内部热切换）。', 'Applied immediately after saving (hot-swapped inside the runtime).'],
  'settings.general.logLevel.saved': ['日志级别已设为 {level}', 'Log level set to {level}'],
  'settings.general.logLevel.trace': ['trace（最详细）', 'trace (most verbose)'],
  'settings.general.logLevel.debug': ['debug', 'debug'],
  'settings.general.logLevel.info': ['info', 'info'],
  'settings.general.logLevel.warn': ['warn', 'warn'],
  'settings.general.logLevel.error': ['error', 'error'],
  'settings.general.logLevel.off': ['off（关闭日志）', 'off (logging disabled)']
} as const
