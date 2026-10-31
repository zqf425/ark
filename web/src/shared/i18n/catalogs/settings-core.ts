/**
 * Settings page shell: page title, appearance preferences, and the nav rail.
 *
 * `settings.nav.*` labels are referenced by `settings-navigation.ts` through
 * `MessageKey`, so renaming one would silently break the rail.
 */
export const SETTINGS_CORE_MESSAGES = {
  'settings.title': ['设置', 'Settings'],
  'settings.description': ['配置本地运行时与外部能力。', 'Configure the local runtime and external capabilities.'],
  'settings.appearance.title': ['外观与语言', 'Appearance & language'],
  'settings.appearance.desc': ['仅保存在当前设备，立即生效。', 'Stored only on this device and applied immediately.'],
  'settings.language': ['显示语言', 'Display language'],
  'settings.fontSize': ['字体大小', 'Font size'],
  'settings.theme': ['主题', 'Theme'],
  'settings.language.zh-CN': ['简体中文', 'Simplified Chinese'],
  'settings.language.en-US': ['English', 'English'],
  'settings.fontSize.small': ['小', 'Small'],
  'settings.fontSize.medium': ['中', 'Medium'],
  'settings.fontSize.large': ['大', 'Large'],
  'settings.theme.light': ['浅色', 'Light'],
  'settings.theme.dark': ['深色', 'Dark'],
  'settings.theme.system': ['跟随系统', 'System'],
  'settings.nav.general.label': ['工作区', 'Workspace'],
  'settings.nav.general.detail': ['模型、路径与日志', 'Models, paths, and logs'],
  'settings.nav.runtime.label': ['运行时', 'Runtime'],
  'settings.nav.runtime.detail': ['回合与子 agent 策略', 'Turns and subagent policy'],
  'settings.nav.providers.label': ['供应商', 'Providers'],
  'settings.nav.providers.detail': ['模型目录与凭据', 'Model catalog and credentials'],
  'settings.nav.integrations.label': ['集成', 'Integrations'],
  'settings.nav.integrations.detail': ['MCP 工具服务', 'MCP tool services'],
  'settings.nav.skills.label': ['Skills', 'Skills'],
  'settings.nav.skills.detail': ['已发现的能力', 'Discovered capabilities'],

  /** Error-boundary labels on the settings page. */
  'settings.boundary.general': ['通用设置崩溃', 'General settings crashed'],
  'settings.boundary.runtime': ['运行调优设置崩溃', 'Runtime tuning settings crashed'],
  'settings.boundary.subagents': ['子 agent 设置崩溃', 'Subagent settings crashed'],
  'settings.boundary.providers': ['供应商设置崩溃', 'Provider settings crashed'],
  'settings.boundary.mcp': ['MCP 设置崩溃', 'MCP settings crashed'],
  'settings.boundary.skills': ['Skills 设置崩溃', 'Skills settings crashed']
} as const
