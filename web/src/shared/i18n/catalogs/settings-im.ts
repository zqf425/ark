/**
 * IM channel section copy (requirements/im-channels.md).
 *
 * Catalog keys only; identifiers are wire values and live with the components.
 */
export const SETTINGS_IM_MESSAGES = {
  'settings.nav.im.label': ['IM 接入', 'IM channels'],
  'settings.nav.im.detail': ['微信 / 飞书 / Telegram', 'WeChat / Feishu / Telegram'],
  'settings.im.title': ['IM 接入', 'IM channels'],
  'settings.im.desc': [
    '把微信、飞书、Telegram 的消息接入 ark 会话；白名单控制谁能触发回合。',
    'Bring WeChat, Feishu and Telegram messages into ark sessions; the whitelist decides who can trigger turns.'
  ],
  'settings.im.create': ['新增渠道', 'Add channel'],
  'settings.im.loadFailed': ['加载 IM 渠道失败', 'Failed to load IM channels'],
  'settings.im.emptyTitle': ['尚未配置 IM 渠道', 'No IM channel configured'],
  'settings.im.emptyHint': ['新增一个渠道，把聊天消息接入 ark。', 'Add a channel to bring chats into ark.'],
  'settings.im.master.label': ['启用 IM 接入（im.enabled）', 'Enable IM channels (im.enabled)'],
  'settings.im.master.hint': [
    '关闭后所有渠道断开，可继续编辑但不建立连接。',
    'When off, every channel disconnects; editing stays available but nothing connects.'
  ],
  'settings.im.master.saveFailed': ['保存 IM 开关失败', 'Failed to save the IM switch'],
  'settings.im.master.rejected': ['IM 开关被拒绝', 'IM switch rejected'],
  'settings.im.master.succeeded': ['IM 开关已更新', 'IM switch updated'],
  'settings.im.kind.telegram': ['Telegram', 'Telegram'],
  'settings.im.kind.feishu': ['飞书', 'Feishu'],
  'settings.im.kind.weixin': ['微信', 'WeChat'],
  'settings.im.state.disabled': ['未连接', 'Disconnected'],
  'settings.im.state.connecting': ['连接中', 'Connecting'],
  'settings.im.state.online': ['在线', 'Online'],
  'settings.im.state.sendOnly': ['仅发送', 'Send-only'],
  'settings.im.state.error': ['错误', 'Error'],
  'settings.im.enabledOff': ['已停用', 'Disabled'],
  'settings.im.enabledLabel': ['启用渠道', 'Enabled'],
  'settings.im.toggleHint': [
    '启用即建立长连接；会话默认无人值守执行。',
    'Enabling opens the long connection; sessions run unattended by default.'
  ],
  'settings.im.toggled': ['渠道 {title} 已更新', 'Channel {title} updated'],
  'settings.im.toggleFailed': ['渠道更新失败', 'Failed to update the channel'],
  'settings.im.deleteConfirm': ['删除渠道 {title}？其绑定会一并删除。', 'Delete channel {title}? Its bindings go with it.'],
  'settings.im.deleted': ['渠道 {title} 已删除', 'Channel {title} deleted'],
  'settings.im.deleteFailed': ['渠道删除失败', 'Failed to delete the channel'],
  'settings.im.verify': ['校验', 'Verify'],
  'settings.im.verify.succeeded': ['凭证有效：{identity}', 'Credentials valid: {identity}'],
  'settings.im.verify.failed': ['凭证校验失败', 'Credential verification failed'],
  'settings.im.verify.identity': ['平台身份', 'Platform identity'],
  'settings.im.saveFailed': ['渠道保存失败', 'Failed to save the channel'],
  'settings.im.created': ['渠道 {title} 已创建', 'Channel {title} created'],
  'settings.im.updated': ['渠道 {title} 已保存', 'Channel {title} saved'],
  'settings.im.nothingChanged': ['没有需要保存的修改', 'Nothing to save'],
  'settings.im.form.createTitle': ['新增 IM 渠道', 'New IM channel'],
  'settings.im.form.editTitle': ['编辑 IM 渠道', 'Edit IM channel'],
  'settings.im.form.kind': ['平台', 'Platform'],
  'settings.im.form.title': ['名称', 'Title'],
  'settings.im.form.allowedChatIds': ['会话白名单（每行一个 chat id）', 'Chat whitelist (one chat id per line)'],
  'settings.im.form.allowedChatIdsHint': [
    '留空表示拒绝所有新会话（对方会收到带其 chat id 的提示）；配置后名单外静默忽略。',
    'Empty rejects every new chat (the sender gets a hint with their chat id); configured lists silently drop outsiders.'
  ],
  'settings.im.form.requireMention': ['群聊必须 @机器人', 'Groups must @-mention the bot'],
  'settings.im.form.workspace': ['工作区覆盖（留空用默认）', 'Workspace override (empty = default)'],
  'settings.im.form.model': ['模型覆盖（留空用默认）', 'Model override (empty = default)'],
  'settings.im.form.providerId': ['供应商覆盖（留空用默认）', 'Provider override (empty = default)'],
  'settings.im.form.autonomy': ['自治级别', 'Autonomy'],
  'settings.im.form.enabled': ['启用', 'Enabled'],
  'settings.im.form.secretsHint': [
    '密文字段不回显；保存时留空表示保留原值。',
    'Secrets are never shown back; saving a blank keeps the stored value.'
  ],
  'settings.im.credentials.telegram.botToken': ['Bot Token', 'Bot token'],
  'settings.im.credentials.telegram.botUsername': ['Bot 用户名（可选）', 'Bot username (optional)'],
  'settings.im.credentials.feishu.appId': ['App ID', 'App ID'],
  'settings.im.credentials.feishu.appSecret': ['App Secret', 'App secret'],
  'settings.im.credentials.feishu.domain': ['域名', 'Domain'],
  'settings.im.credentials.feishu.domainFeishu': ['飞书 (feishu.cn)', 'Feishu (feishu.cn)'],
  'settings.im.credentials.feishu.domainLark': ['Lark (larksuite.com)', 'Lark (larksuite.com)'],
  'settings.im.credentials.weixin.botToken': ['Bot Token', 'Bot token'],
  'settings.im.credentials.weixin.baseUrl': ['API 地址（可选）', 'API base URL (optional)'],
  'settings.im.credentials.weixin.ownerUserId': ['绑定用户 ID', 'Owner user ID'],
  'settings.im.bindings.label': ['会话绑定', 'Bindings'],
  'settings.im.bindings.empty': ['暂无绑定', 'No bindings yet'],
  'settings.im.bindings.openThread': ['打开会话', 'Open thread'],
  'settings.im.login.title': ['微信扫码连接', 'Connect WeChat via QR'],
  'settings.im.login.qrHint': ['用手机微信扫描以下二维码：', 'Scan this QR code with WeChat:'],
  'settings.im.login.confirmed': ['微信渠道已创建（默认停用）', 'WeChat channel created (disabled by default)'],
  'settings.im.boundary': ['IM 接入区块渲染失败', 'IM section failed to render']
}
