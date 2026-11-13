# IM 渠道接入（IM Channels）

> 状态:**已实现**(2026-11-02 设计定稿,五个阶段全部落地,离线可测部分;真实 Telegram/飞书/微信
> 平台端到端由发布验收承接,本机未执行)。设计节即实现语义,落地情况见文末。参照 smart-ark 桌面端
> IM 接入(内部代号
> claw,`src/main/claw-runtime.ts` 及 feishu/telegram/weixin 三个平台
> runtime)按 ark 架构
> 裁剪。QQ、企业微信等新渠道按同一扩展点后续另立项,不在本需求范围。

## 背景与目标

smart-ark 在桌面端主进程实现了 IM 接入:外部 IM 平台(飞书/Telegram/微信)的消息驱动一个
kun 会话回合,回复经平台 API 推回聊天。ark 作为重写版需要等价能力:用户在 arkd 配置一个
IM 渠道(凭证+策略),对应平台上的私聊/群聊消息自动映射到 ark 会话线程并触发真实回合,
助手回复推回 IM;渠道可启停、可增删,后续可扩展新平台。

对标结论(取自 smart-ark,按 ark 架构裁剪):

| smart-ark 概念 | 本需求取舍 |
|---|---|
| Electron main 进程内平台 runtime(claw-runtime 等) | arkd 进程内 IM 服务:编排居 `ark-server` service 层,平台适配居新 crate `ark-im` |
| 经 HTTP 自调用 kun 建线程+回合 | 进程内复用 `start_turn` 全部校验路径(同定时任务先例) |
| `approvalPolicy:'auto'` + `disableUserInput` | 线程 runtime `autonomy=unattended` 覆盖(gate 永不等待不在场的人) |
| 飞书流式卡片(`feishu-streamer.ts`) | v1 整段回复;流式卡片为非目标 |
| IM↔桌面会话镜像(mirror coordinator) | 非目标,后续需求另立 |
| 斜杠命令全集(列表/绑定/切模型/定时任务检测) | 仅保留 `/new`(重置会话);其余非目标 |
| 飞书 OAuth 设备码注册 app | v1 用户在渠道表单手工填 appId/appSecret;设备码流为非目标 |
| 微信经 `@tencent-weixin/openclaw-weixin` npm 插件 | 原生实现 iLink bot 协议(长轮询+发送+二维码登录),npm 包仅作协议参照、不作运行时依赖;媒体 CDN 上传下载为非目标 |
| Telegram `net.fetch` 长轮询 | 原生 `getUpdates` 长轮询(零新增依赖,reqwest 已在 workspace) |
| 渠道/会话绑定存应用设置 JSON | 独立 `<root>/IM/im.json`(与渠道配置同域、需原子级联删除,不进 config 版本机制) |

## 范围

### 数据模型(wire 契约,ark-contract `im.rs`)

类型形状沿用 schedules 先例:落盘与服务端内部形状 `ImChannel`、创建载荷
`CreateImChannelRequest`(服务端生成 `id`/`created_at`/`updated_at`)、合并语义更新载荷
`UpdateImChannelRequest`(每字段"提供即覆盖,缺省即保留";**密文字段为空串/缺省视为
保留原值**,与 provider 的 `api_key` 空值回填做法一致——否则只改标题的编辑表单会把
已存密钥清空)、读模型 `ImChannelView`。游标是服务端维护的落盘运行态,不进任何 wire
模型。

```text
ImChannelId      # newtype(string,与 ThreadId/AttachmentId 同风格);ImChannel.id、
                 # ImBinding.channel_id、InboundMessage.channel_id 统一用它
ImChannelKind    # telegram | feishu | weixin
ImCredentials    # 按 kind 判别的枚举(全量,仅写/落盘路径):
                 #   telegram { bot_token, bot_username? }
                 #   feishu   { app_id, app_secret, domain: feishu|lark }
                 #   weixin   { bot_token, base_url?, owner_user_id }   # owner 必填
ImCredentialsView # 脱敏读模型,不复用写 DTO(同 ProviderView 命名先例):
                 #   仅含非密字段(telegram.bot_username / feishu.app_id+domain /
                 #   weixin.base_url+owner_user_id)+ 各密文字段的 has_* 布尔
ImChannelPolicy {
  allowed_chat_ids: []     # 会话白名单。空白名单 = 不响应任何新会话,但向发起者回复
                           # 提示(附上其 chat_id,供用户复制进白名单;首次接入引导,
                           # 已绑定会话不受影响)。白名单非空时,名单外会话一律静默
                           # 丢弃(不提示——避免向任意第三方发消息)。
                           # weixin 渠道额外按 owner_user_id 过滤 sender。
  require_mention: bool    # 群聊必须 @bot 才响应;默认 true(serde default + 归一化
                           # 回退双保险)。v1 仅飞书群有群聊,Telegram v1 仅私聊
}
ImChannel {                  # 落盘与服务端内部形状;出现在 im.json,不出现在任何
  id, kind, title, enabled   # 请求体(kind 创建后不可变,凭证形状随 kind)
  credentials
  policy
  workspace?                 # 覆盖会话工作区;空 = 全局默认
  provider_id?, model?       # 覆盖;空 = 运行时默认
  autonomy?                  # 默认 unattended
  created_at, updated_at
}
CreateImChannelRequest {     # POST 载荷,不含 id/created_at/updated_at
  kind, title, enabled, credentials, policy,
  workspace?, provider_id?, model?, autonomy?
}                            # (与 CreateScheduleRequest 同构)
UpdateImChannelRequest {     # PATCH 合并语义,每个字段都是"提供即覆盖,缺省即保留";
  title?, enabled?,          # 不含 id/kind/created_at;credentials 密文字段空=保留
  credentials?, policy?, workspace?, provider_id?, model?, autonomy?
}
ImChannelView {              # 脱敏读模型(同 ImChannel 形状,credentials 换 View,
  ...同 ImChannel            # 不含游标等运行态);GET/PATCH 响应用它
  credentials: ImCredentialsView
}
ImChannelListItem {          # GET /v1/im/channels 列表项:渠道读模型 + 只读运行态
  channel: ImChannelView,
  runtime: ImChannelRuntime
}
ImChannelRuntime {           # 只读运行态,不落盘
  state: disabled|connecting|online|send_only|error
                             # send_only:飞书降级态(能发不能收),避免误显示 online
  last_error?, last_inbound_at?
}
ImBinding { channel_id: ImChannelId, chat_id, thread_id, updated_at }
ImSettings { enabled: false }# config.im,ReloadClass::Hot,总开关只门控连接不门控
                             # CRUD。默认 false 是刻意为之(与 schedules 相反):
                             # IM 渠道触发 unattended 回合在本机执行工具,需用户
                             # 主动开启,避免升级后自动连上已配置渠道
```

落盘记录类型 `ImChannelRecord { channel, cursor: Option<String> }` **居 `ark-ports`**
(与 `ThreadMeta`/`ChildRunRecord` 先例同宿主:`ImStore` 端口签名要命名它;不登记
`ts_export`,不出现在 `web/src/generated`)。`cursor` 为服务端入站游标(telegram update
offset 等平台游标,统一以 `Option<String>` 落盘,offset 用十进制串),
GET 不回传,客户端不可写。

- 渠道数上限 `MAX_IM_CHANNELS = 20`(常量居 ark-contract,与 schedules 常量同宿主);
  容量校验 `ensure_channel_capacity` 居 `ark-im`(同 `ark-schedule::ensure_capacity`
  先例),超限拒绝创建(`validation_error`)。微信登录自动建渠道路径同样经该校验,
  超限时登录失败并返回结构化错误,不创建第 21 个渠道。
- 凭证沿用 provider `api_key` 的 v1 简化:明文落盘 `<root>/IM/im.json`;读接口一律换
  `ImCredentialsView`(web 只写不回读)。迁 OS keyring 是全局后续事项,不在本需求。
- **安全默认**:渠道 `autonomy` 缺省 `unattended` 意味着 IM 触发的回合在本机无人值守执行
  工具;因此白名单默认拒绝(空 = 不放行新会话)与总开关默认关闭都是刻意的安全默认值,
  任何"配好凭证即对未知第三方开放工具执行"的组合都不允许出现。
- 归一化:标题截断、`allowed_chat_ids` 去重、凭证按 kind 校验必填(`owner_user_id` 缺失
  即无效);修复优先策略与 ark-schedule 一致,但**凭证缺失是结构化错误**(创建/启用无效
  凭证的渠道应被拒绝并提示,而不是静默起一个必然失败的长连接)。
- 落盘复用 wire 形状沿用 `schedules.json` 先例:`ark-contract` 承载的是形状与上限常量,
  不承载持久化行为;`im.json` 的读写行为居 `ark-store`。

### 会话映射与绑定

- 会话键:`(channel_id, chat_id)`;`chat_id` 为平台会话标识(Telegram chat id、飞书
  chat_id/open_id、微信 user_id)。绑定 `ImBinding` 持久化在 `im.json`,首条消息触发
  `create_thread` 后写入;线程丢失(404)时自动解绑重建。
- 线程标题取首条消息截断(上限 40 字符)。来源追溯经 `im.json` 绑定关系;线程权威记录
  (turn_started)不携带 IM 来源,v1 不做来源字段的契约扩展。线程 runtime 覆盖
  `autonomy`(渠道字段,缺省 `unattended`)、`workspace`/`provider_id`/`model`。
- **入站文本落为持久用户消息**:IM 消息必须走 `TurnSource::Im` 并经
  `apps/arkd/src/wiring/turns.rs::turn_source_texts` 映射为用户消息写进权威日志
  (历史回放、`/new` 语义、回合种子都依赖这一点);新增 `TurnSource` 变体时同步补齐
  该 match(阶段 1)。
- `/new` 命令:解绑当前线程并新建(下条消息触发建线),回复确认;未识别的 `/` 命令回复
  简短提示。

### 入站管线(ark-server `service/im.rs` 编排,平台无关)

1. **过滤**:bot 自身消息、白名单外会话(空白名单按安全默认拒绝并提示;非空白名单外的
   静默丢弃)、群聊未 @(`require_mention` 生效时)、weixin 非 owner sender、空文本
   一律丢弃,不触发回合。
2. **去重**:每渠道有界 LRU `message_id` 集(内存,`DEDUP_CAPACITY = 1024`);跨重启的
   重复投递由持久化游标收敛。游标落盘**节流**(每渠道每 10 条或每 30 秒,取先)且
   **不阻塞入站 select 循环**(独立写任务或 `spawn_blocking`),避免每条消息重写整份
   `im.json`;写失败只记 `runtime.last_error`,不丢消息。已知限制:游标写入与回合启动
   之间的崩溃窗口内,窗口内消息重启后可能重复触发回合(上限即节流窗口 10 条),列入
   验收的已知限制。
3. **会话映射与建线**:见上节。
4. **派发**:每 conversation 串行——该会话已有回合在跑时入队(每会话队列上限 4,溢出
   回复 busy 提示);不阻塞同渠道其它会话(一个回合可跑数分钟)。队列只串行化 IM 自己
   派发的回合;桌面端正在该线程跑回合时 `start_turn` 返回 `Conflict`(409),此时按
   `IM_BUSY_RETRY_SECS = 30` 有界重试(至多 5 次),耗尽后回复 busy 提示——队列与
   `state.turns` 的关系是"队列消解 IM 侧并发,registry 是唯一冲突仲裁"。
5. **回合**:`start_turn(state, thread_id, text, [])`;文本超 `MAX_TURN_TEXT_BYTES` 时
   截断到上限。不持有 `workspace_gate` 做任何长耗时动作(红线 §7.5)。

### 回复路径

- **先订阅后发起**:`store.subscribe(thread_id, since_seq=当前水位)`(与 SSE 复用同一
  `SessionStore::subscribe` 临界区,不开第二个订阅入口;`AppState` 不新增 events 句柄),
  拿到水位后再 `start_turn`;事件一律按 `turn_id` 过滤,避免把上一回合的 assistant 文本
  累积进本次回复(红线 §7.6 不用 `since_seq=0` 重放)。
- 订阅**仅用于探测终态**;最终回复文本在终态事件到达后取**权威投影**——按本回合
  `turn_id` 过滤的最后一条非空 assistant 文本;本回合无 assistant 文本(失败/中止)时
  按终态分支发失败/中止提示,**不回退到历史回合文本**(投影保证 lag 时回复仍完整,
  turn_id 过滤保证不会把上一回合的答案发给用户)。
- **超时解耦**(对标 smart-ark `scheduleImResultPush`):回合超过 `IM_ACK_SECS = 120` 未
  完成先发 ack("正在处理…"),后台继续等待至多 `IM_MAX_WAIT_SECS = 1800`,超时回复失败
  提示并放弃等待(回合本身继续跑到终态,结果留在会话线程里)。
- 发送按平台上限分块(Telegram 4096 字符等);发送失败降级为截断纯文本,再失败记录
  `runtime.last_error`。
- v1 不回传工具过程/推理内容,只回传最终 assistant 文本。

### HTTP API(ark-server)

- `GET /v1/im/channels` 列表(`ImChannelListItem[]`,凭证脱敏,不含游标);
  `POST /v1/im/channels` 创建(载荷 `CreateImChannelRequest`,响应 `ImChannelView`);
  `PATCH /v1/im/channels/{id}` 更新(载荷 `UpdateImChannelRequest`,合并语义,响应
  `ImChannelView`);`DELETE /v1/im/channels/{id}` 删除——删除即解绑并释放 transport;
  在途回合不中断,其回复因发送目标消失而丢弃并记 `last_error` 一次(回合终态语义
  不受影响)。
- `POST /v1/im/channels/{id}/verify` 校验凭证(Telegram `getMe`、飞书取
  tenant_access_token、微信校验 token 拉取一次更新),返回平台身份描述或结构化错误。
- `GET /v1/im/bindings` 绑定列表(按 `channel_id` 过滤,只读)。
- 微信二维码登录编排(阶段 4):`POST /v1/im/weixin/login/start` 返回二维码内容,
  `GET /v1/im/weixin/login/wait` 轮询登录状态;登录成功后由**后端直接创建渠道**(经
  `ensure_channel_capacity`,超限即登录失败)——title 取平台身份昵称、policy/autonomy
  取默认值、`enabled=false`(用户编辑后启用),返回新建的 `ImChannelView`。不先建空渠道
  (凭证缺失是结构化错误),也不让凭证经前端中转。登录流由 arkd 装配的独立对象持有
  (**不依赖已解析凭证**——凭证恰恰是登录的产物),`LoginSession` 是进程内短生命周期
  状态(10 分钟过期、同时至多 1 个登录会话),不落盘。
- 全部经 wire DTO 生成前端类型(`ts_export`);错误复用 `ArkErrorCode` 语义
  (不存在 404、超限 `validation_error`/400;回合冲突 409 不外露——IM 侧按上节重试
  消化)。

### 配置(web 设置页)

- 设置页新增「IM 接入」section:渠道列表(平台、标题、启停、连接状态、最近错误)、
  新建/编辑表单(按平台渲染凭证与策略字段;密文字段不回显,空=保留原值)、校验按钮、
  绑定列表(只读,可打开对应线程)、微信二维码登录对话框(登录成功后展示新建渠道并
  进入编辑)。
- 总开关 `im.enabled`(默认关)关闭时所有渠道断开,列表可查看可编辑,但不建立连接
  (同 schedules 先例)。
- 全部文案走 `web/src/shared/i18n/` 消息键与中英文资源。

## 模块落位

| 职责 | 宿主 | 依据 |
|---|---|---|
| 渠道/绑定/凭据/策略 wire DTO + ts_export | `ark-contract`(`im.rs`) | Rust 类型是 wire 唯一真源 |
| `ImStore` 端口(trait,**同步**,与 `ScheduleStore` 同性质:整份文档一次进程内锁临界区读写,锁内不做网络 IO)与 `ImChannelRecord` 落盘记录类型(不登记 ts_export) | `ark-ports` | 端口签名要命名落盘记录(同 `ThreadMeta` 先例);实现由组合根注入,编排不认识文件系统 |
| `ImFileStore`(`im.json`,Mutex 串行 + tmp+rename 原子写) | `ark-store` | 与 ScheduleFileStore 同模式 |
| 归一化消息模型、`ImTransport` trait、三平台适配器、去重/分块/命令纯函数、容量校验 | 新 crate `ark-im` | 长连接网络 IO 无法进插件(无 WS/监听能力),也不能进 `ark-server`(它会反向依赖适配器实现) |
| 入站编排、绑定、回合驱动、回复订阅、HTTP 路由 | `ark-server`(`service/im.rs` 等) | 与 scheduler 同层,复用 `start_turn`/`SessionStore::subscribe`/gate 语义 |
| 装配(构建适配器与登录流对象、注入经治理的 HTTP client、注册路由、连接循环启动、shutdown 挂钩) | `apps/arkd` | 唯一组合根;适配器与登录流的 `reqwest::Client` 由组合根经 `ark_http::transport_client`(同一 `HttpPolicy` 的代理/TLS 设置)构造注入,IM 不自建裸 client |
| 管理界面 | `web` 设置页 | 只消费生成契约 |

依赖方向新增:`ark-im → ark-common, ark-contract, ark-ports`;`ark-server → ark-im`;
其余不变,无循环。控制原语(`CancelToken`)取自 `ark-ports::exec`,不经 `ark-agent`;
出站 HTTP 治理沿用 ark-http 先例(模型与目录流量同款),`ark-im` 不引入第二条出站通道。

### 文档同步清单(随对应阶段一并提交,不得只改代码)

| 阶段 | 需同步更新的文档 |
|---|---|
| 阶段 1 | `docs/modules/ark-contract.md`(im.rs 与根类型登记)、`docs/modules/ark-ports.md`(ImStore 端口与 `ImChannelRecord` 端口类型,后者不登记 ts_export)、`docs/modules/ark-store.md`(ImFileStore)、`docs/modules/ark-config.md`(config.im 段) |
| 阶段 2 | `docs/modules/ark-server.md`(service/im.rs 与路由)、`docs/modules/arkd.md`(装配与 shutdown 挂钩)、`docs/rust-rewrite-design.md` §5 依赖表(ark-im 行、ark-server 行)、本文状态与落地情况、`docs/modules/ark-im.md` 状态 |
| 阶段 3/4 | 本文与 `docs/modules/ark-im.md` 的平台小节(落地偏差如实回填) |
| 阶段 5 | `docs/modules/web.md`(设置页 section) |

## 非目标(后续需求另立项)

- QQ、企业微信、Slack、WhatsApp 等新渠道(扩展点已预留:`ImChannelKind` + `ImTransport`
  实现 + ark-contract 凭证枚举分支)。
- 飞书流式卡片、入站/出站媒体文件(图片/文件经 `AttachmentStore` 接线)、语音。
- IM↔桌面会话镜像、斜杠命令全集、飞书 OAuth 设备码注册、定时任务自然语言检测。
- turn_started 携带 IM 来源的契约扩展。
- 多 agentProfile/人设 prompt;凭证 OS keyring;im.json 多写进程协调(单写进程红线不变)。

## 验收

- 单元:ark-im 归一化/分块/去重 LRU 有界/命令解析/凭证按 kind 必填校验/容量校验;
  ark-server 假 transport 下入站→白名单(空提示/非空静默)/去重/队列→绑定→`start_turn`→
  终态→按 turn_id 投影取文本→`send_text` 回发全链路(含失败/中止回合的终态提示分支),
  @提及、游标节流恢复、409 有界重试、超时 ack 分支,渠道 CRUD/上限/verify 路由,
  `turn_source_texts` 对 `TurnSource::Im` 的用户消息映射。
- 契约:`cargo test -p ark-contract --lib ts_export` 后 `web/src/generated` 零 diff。
- 质量:`cargo fmt --check`、`cargo check`、`cargo clippy -D warnings`(受影响包),
  `cargo test --workspace --exclude ark-desktop`;`dependency_direction.rs` 含 ark-im 新行。
- 端到端(发布验收):真实 Telegram bot 私聊发消息→自动建线完成回合→回复送达;渠道停用
  后不再响应;重启后绑定与游标恢复,重复回合以节流窗口为上限(已知限制);阶段 3/4
  各自的降级判定验收(飞书长连接不可用时按预设标准切 `send_only` 或移出范围;微信
  协议受阻时按退出条件处理)。

## 实施阶段

1. **阶段 1(契约与存储)**:`ark-contract/im.rs`(含 `ImChannelId` newtype、Create/Update
   载荷对与 View 读模型)+ ts_export 根类型登记;`ark-ports::ImStore`(同步端口)+
   `ImChannelRecord`(端口类型,不登记 ts_export);`ark-store::ImFileStore`;
   `TurnSource::Im` 变体 + `turn_source_texts` 映射;`config.im` 热更新段(触点:
   `ark-contract/src/config.rs` 的 `ImSettings`/`ConfigPatch.im`/`RuntimeConfigView.im`,
   `ark-config` 的 `classify.rs`/`diagnostics.rs`/`snapshot.rs`,参照 schedules 四处
   落点)。同步文档清单第 1 行。
2. **阶段 2(编排与 Telegram)**:`ark-im` 域模型 + `ImTransport` + Telegram 适配器
   (HTTP client 由组合根经 `ark_http::transport_client` 注入,长轮询以
   `client_timeout=None` 构造、单请求超时由适配器按平台轮询窗口自行设界);
   `ark-server` 入站编排/回复路径/HTTP 路由;arkd 装配;dependency_direction 表更新。
   同步文档清单第 2 行。
3. **阶段 3(飞书)**:WebSocket 长连接适配器(官方长连接协议自实现,tokio-tungstenite,
   TLS 启用 `rustls-tls-webpki-roots`,与 reqwest 的 rustls 同栈)。降级判定:协议细节
   受阻且短期内不可解时,渠道进入 `send_only`(仅保留 REST 发送、入站暂不支持)并如实
   回填文档——webhook 入站需要公网回调入口,arkd 是 loopback 边界,不作为降级目标
   (未来若提供带签名校验与 challenge 握手的回调入口,另立项)。
4. **阶段 4(微信)**:iLink bot 协议长轮询适配器 + 二维码登录编排(`ImLoginFlow`,
   同样注入经治理的 client)。
   退出条件:协议逆向不可行或鉴权不稳定时,微信移出 v1 范围、更新非目标并回填文档。
5. **阶段 5(管理界面)**:设置页 section + i18n。同步文档清单第 5 行。

## 落地情况

阶段 1(契约与存储)已实现:`ark-contract/im.rs`(含读/写模型分离、`UpdateImChannelRequest`
合并语义、凭证空值保留规则)与 ts_export 根类型登记;`ark-ports::ImStore`(同步端口)+
`ImChannelRecord`(不登记 ts_export);`ark-store::ImFileStore`(原子写、损坏恢复、渠道删除
级联绑定);`TurnSource::Im` 变体与 `start_turn_as`/`turn_source_texts` 映射;`config.im`
热更新段(contract `ImSettings`/`ConfigPatch.im`/`RuntimeConfigView.im` + ark-config 的
classify/diagnostics/snapshot 落点,默认关)。

阶段 2(编排与 Telegram)已实现:`ark-im` crate(归一化消息模型、`ImTransport` trait、
create/update 载荷归一化、去重 LRU、分块、命令解析、容量校验、`build_transport` 工厂,
13 个单测)与 Telegram 适配器(getUpdates 长轮询、指数退避、HTML 转义与纯文本降级、游标
上报、`is_self`);`ark-server` 的 `service/im.rs`(sync 对账/渠道启停/游标节流落盘)与
`service/im_inbound.rs`(白名单空提示/非空静默、@提及、weixin owner、去重、每会话串行
队列、`TurnSource::Im` 回合、订阅探测终态 + 注册表轮询兜底、按 turn_id 投影取回复、
超时 ack、busy 有界重试);HTTP 路由 `/v1/im/channels`(CRUD/verify)与 `/v1/im/bindings`;
arkd 装配(`ImFileStore` + 经治理的 transport client + `im.spawn`,仅随 serve 启动)。
测试:ark-server 173 用例(含 7 个 IM 编排集成测试)、ark-im 13 用例;依赖方向表已含
`ark-im` 行;文档同步清单第 1、2 行均已回填。

阶段 3(飞书)已实现:`ark-im/src/feishu.rs` WebSocket 长连接,协议自官方
`@larksuiteoapi/node-sdk`(smart-ark 的参照实现)原生提取:`POST /callback/ws/endpoint`
拉取签名 URL 与 PingInterval 等客户端配置、pbbp2.Frame protobuf 帧手写编解码(SeqID/
LogID/service/method/headers/payload 九字段)、ping/pong 心跳、事件按 message_id/sum/seq
分片重组(10s TTL)、echo ack 回帧(`{"code":200}` base64);REST 侧 tenant_access_token
缓存刷新、`im/v1/messages` 文本发送(按 kind 上限分块)、`bot/v3/info` 身份与 `is_self`。
群聊 `is_mention` 由事件 `mentions` 非空判定,`require_mention` 策略在编排层生效。降级
判定未触发:协议完整提取,无需 `send_only`。4 个单测(帧编解码回环、未知字段跳过、分片
重组、事件归一化)。飞书无重放游标(网关在带内 ack,重投递由网关重试窗口与去重集合兜底),
`cursor()` 恒为 `None`。

阶段 4(微信)已实现:`ark-im/src/weixin.rs` iLink bot 协议原生实现(参照 smart-ark
`weixin-bridge-runtime.ts` 与 `@tencent-weixin/openclaw-weixin` 2.4.3,不引 Node 依赖):
`getupdates` 长轮询(游标 `get_updates_buf` 随渠道持久化)、`message_type==2` 自过滤、
TEXT/VOICE 文本提取、`context_token` 按发送者缓存(主动推送凭据)、`sendmessage`
(BOT/FINISH/TEXT,分块)、`ret`/`errcode` 业务错误检查、连续失败退避(2s/30s)。二维码登录
`WeixinLoginFlow`(`get_bot_qrcode` + `get_qrcode_status` 状态机 wait/scaned/expired/
confirmed;`need_verifycode`/`verify_code_blocked` 显式失败):HTTP 路由
`POST /v1/im/weixin/login/start` 与 `GET /v1/im/weixin/login/wait`;确认后由后端直接创建
渠道(经容量校验,title 取 owner id,`enabled=false` 凭证不落前端),wire 新增
`ImWeixinLoginStatus`/`StartResponse`/`WaitResponse`。`ImLoginFlow` trait 改为
async-trait(`start` 需网络 IO)。3 个单测(文本提取、业务错误、confirmed 映射)。
退出条件未触发:协议完整可用。

阶段 5(管理界面)已实现:设置页新增「IM 接入」section——渠道列表(平台/标题/启停/
连接状态/最近错误/校验身份/绑定列表可打开线程)、新建编辑表单(按平台渲染凭证与策略,
密文不回显、空白=保留)、微信扫码对话框(取码→2s 轮询→确认后展示新建渠道)、总开关
`im.enabled`(默认关,只门控连接)。API client `im-api.ts`、hook `use-im.ts`、表单映射
`im-form.ts`(凭证变更检测:非空密文即变更,空白密文保留原值,非密字段直接比对;4 个
单测);i18n 新增 `settings-im` 目录(中英双语文案,导航与 settings-navigation 测试同步)。
web 142 用例全绿,typecheck 与硬规则(当时单文件上限 400 行,ImSection 拆分为
ImChannelForm/WeixinLoginDialog;上限后放宽至 1000 行)通过。

至此五个阶段全部落地。真实平台端到端(真实 Telegram bot/飞书应用/微信扫码)属发布验收,
本机未执行。

**2026-11-03 补强(扫码渲染)**:微信扫码对话框此前把 `qr_content` 当纯文本放进 `<code>`,
用户看到的是登录 URL 字符串而非可扫的二维码(实测 `get_bot_qrcode` 返回的
`qrcode_img_content`/`qrcodeUrl` 是 `https://liteapp.weixin.qq.com/q/...` 形态的登录
链接)。对齐 smart-ark 渲染策略(`SidebarClawDialogStepContent` 的
`qrValueIsImage` 双路径):`data:image/` 前缀直接以 `<img>` 展示,其余内容用
`qrcode.react` 编码为二维码(白底容器保证深色主题下可扫);新增 3 个对话框渲染单测。
依赖变更:web `dependencies` 新增 `qrcode.react@^4.2.0`(与 smart-ark 同版本,无传递
依赖,约 10KB gzip 量级进主 chunk)。

