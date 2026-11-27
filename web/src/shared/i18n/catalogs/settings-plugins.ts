/**
 * Settings -> Plugins & capabilities: the runtime diagnostics (capabilities,
 * plugin host report, built-in tool catalog) plus the enable flow for plugins
 * that declare a `provision` section.
 */
export const SETTINGS_PLUGINS_MESSAGES = {
  'settings.plugins.capabilities.title': ['运行时能力', 'Runtime capabilities'],
  'settings.plugins.capabilities.desc': ['当前构建实际提供的能力；页面与面板按所需能力过滤。', 'Capabilities this build actually provides; pages and panels are filtered by them.'],
  'settings.plugins.capabilityCatalog': ['注册页面 {pages} 个，面板 {panels} 个。', '{pages} page(s) and {panels} panel(s) registered.'],

  'settings.plugins.title': ['插件', 'Plugins'],
  'settings.plugins.desc': ['已加载插件及其向页面、面板和工具总线的贡献；只读。', 'Loaded plugins and their contributions to the page, panel and tool buses; read-only.'],
  'settings.plugins.loadFailed': ['读取失败', 'Failed to load'],
  'settings.plugins.empty': ['当前没有已加载插件。', 'No plugins are loaded.'],
  'settings.plugins.brokenTitle': ['存在加载异常的插件', 'Some plugins failed to load'],
  'settings.plugins.brokenBody': ['register 失败：{failed}；因依赖缺失跳过：{skipped}。', 'Register failed: {failed}. Skipped for missing dependencies: {skipped}.'],
  'settings.plugins.contributions': ['页面 {pages} · 面板 {panels} · 工具 {tools}', 'Pages {pages} · panels {panels} · tools {tools}'],

  'settings.plugins.tools.title': ['工具目录', 'Tool catalog'],
  'settings.plugins.tools.desc': ['内置工具注册表的快照，展开查看参数 schema；只读。', 'A snapshot of the built-in tool registry; expand an entry for its schema. Read-only.'],
  'settings.plugins.toolsCount': ['共 {count} 个工具（注册表版本 {version}）。', '{count} tools (registry version {version}).'],

  'settings.plugins.provision.desc': ['声明了 provision 段的插件可在此启用：自动检测本机安装；未安装时经你确认后安装；成功后注册其 MCP 服务。未启用前插件不可用。', 'Plugins declaring a provision section can be enabled here: the host detects the local install, installs behind your consent, then registers their MCP server. The plugin stays unusable until enabled.'],
  'settings.plugins.provision.state.disabled': ['未启用', 'Disabled'],
  'settings.plugins.provision.state.enabled': ['已启用', 'Enabled'],
  'settings.plugins.provision.state.needs_install': ['未安装', 'Not installed'],
  'settings.plugins.provision.state.failed': ['启用失败', 'Failed'],
  'settings.plugins.provision.enable': ['启用', 'Enable'],
  'settings.plugins.provision.installAndEnable': ['未检测到本机安装，点击安装并启用', 'Not found locally — click to install and enable'],
  'settings.plugins.provision.retry': ['重试', 'Retry'],
  'settings.plugins.provision.busy': ['处理中…', 'Working…'],

  'settings.plugins.install.title': ['安装插件', 'Install a plugin'],
  'settings.plugins.install.desc': [
    'arkd 在启动时读取插件目录，本版本没有热装载：把插件文件夹放入下列目录后重启 arkd 即生效，卸载同理。',
    'arkd reads the plugin directory at startup and has no hot-reload in this build: drop a plugin folder into the directory below and restart arkd to load it; removing one works the same way.'
  ],
  'settings.plugins.install.path': [
    '插件目录是 arkd 启动日志中 “loaded local plugins” 一行记录的 root；本版本没有通过 API 暴露它。',
    'The plugin directory is the root printed on the “loaded local plugins” startup log line; this build does not expose it through the API.'
  ],
  'settings.plugins.install.required': [
    '每个插件是一个目录，至少包含 plugin.json（id、version、kind；工具在 mcp.expose 声明）。',
    'Each plugin is a directory containing at least plugin.json (id, version, kind; tools are declared in mcp.expose).'
  ],
  'settings.plugins.install.restart': [
    '放入后重启 arkd，本页刷新即可看到新插件的页面、面板与工具贡献。',
    'Restart arkd after dropping it in, then refresh this page to see the new plugin’s pages, panels and tools.'
  ]
} as const
