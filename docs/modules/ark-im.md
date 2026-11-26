# ark-im 模块设计

> 状态:**已实现**(设计已批准,2026-11-02;阶段 1-5 全部落地,需求边界与验收见需求文档
> 「落地情况」)。durable inbox 审计补强已落地:适配器经 `InboundSender` 批量原子落盘,
> 收到 server 的持久入队确认后才推进平台游标;飞书群聊只识别对 bot `open_id` 的提及。
> `ImTransport` 另有 `is_self` 默认方法(飞书/微信需要回显自身消息过滤);
> `ImLoginFlow::start` 为 async(取二维码需网络 IO)。

## 职责与边界

`ark-im` 是 IM 渠道接入的平台适配域:平台中立的归一化消息模型、`ImTransport` 传输适配器
trait 及 Telegram/飞书/微信三个实现、去重/分块/命令解析等纯函数、二维码登录能力接口。
它负责"把平台字节流变成归一化入站消息,把出站文本变成平台消息"。

它不认识会话线程、回合、存储与配置快照——绑定、路由、回合驱动、回复编排都居
`ark-server` 的 `service/im.rs`(与 scheduler 同层);持久化经 `ark_ports::ImStore` 端口,
实现在 `ark-store`。回合决策永远只在 `ark-agent` reactor(单一决策循环不变量)。

依赖:`ark-common`、`ark-contract`、`ark-ports`(控制原语 `CancelToken` 取自
`ark-ports::exec`,不经 `ark-agent`),外加 `tokio`、`async-trait`(trait 需要)、
`reqwest`(Telegram/微信长轮询与飞书 token 获取;workspace 已有)、`chrono`(按需)、
`tokio-tungstenite`(飞书 WebSocket 长连接,阶段 3 引入,TLS 启用
`rustls-tls-webpki-roots`,与 reqwest 的 rustls 同栈、不并立第二套 TLS)。不依赖任何
实现 crate,也不被实现 crate 依赖——`ark-server` 只消费其公开 API;依赖方向表
(`dependency_direction.rs` 与整体架构 §5)随阶段 2 同步更新。

**出站 HTTP 治理**:本 crate 不自行创建 `reqwest::Client`——各适配器与登录流
(`ImLoginFlow`)的 client 由组合根经 `ark_http::transport_client`(同一 `HttpPolicy` 的
代理/TLS 设置,与模型/目录流量同款)构造后注入构造函数;需要代理才能出网的环境里渠道
才能正常工作,且这是 arkd 既有治理先例的延伸,不开第二条出站通道。长轮询渠道
(Telegram `getUpdates`、微信、飞书 token 刷新)的 client 以 `client_timeout=None`
构造(该参数是整个请求含 body 读取的上限,长轮询属流式调用方),单请求超时由适配器按
平台轮询窗口自行设界(如 25s+余量)。

## 类型:不重复定义

`ImChannelKind`、`ImChannelId`、`ImCredentials`(写模型)、`ImCredentialsView`(脱敏读
模型)、`ImChannel`(落盘与服务端内部形状)、`CreateImChannelRequest`/`UpdateImChannelRequest`
(载荷对)、`ImChannelView`(读模型)、`ImChannelListItem`、`ImChannelPolicy`、`ImBinding`
等落盘与 wire 形状
都在 `ark-contract::im` 定义,本 crate `pub use` 转出——`im.json`、HTTP 载荷和
`web/src/generated` 三者由契约 crate 统一生成,这里再写一份同形状结构正是漂移的开始
(同 ark-schedule 先例;契约只承载形状与上限常量,不承载持久化行为)。

落盘运行态类型 `ImChannelRecord { channel, cursor: Option<String> }`、`ImInboxEntry`、
`ImInboxId` 与归一化入站消息 `InboundMessage` 都居 **`ark-ports`**(与
`ThreadMeta`/`ChildRunRecord` 同宿主:`ImStore` 端口签名要命名它们),不登记 `ts_export`、
不出现在 `web/src/generated`;`cursor` 是服务端入站游标(统一 `Option<String>`,telegram
update offset 以十进制串存入),不进任何 wire 模型、GET 不回传、客户端不可写。
本 crate 从端口转出 `InboundMessage`,不维护第二份形状。

```rust
/// 平台归一化入站消息(端口类型,本 crate 转出)。所有适配器都折算到这个形状,
/// 编排层不出现平台名。
pub struct InboundMessage {
    pub channel_id: ImChannelId,
    pub chat_id: String,       // telegram chat id / feishu chat_id|open_id / weixin user_id
    pub sender_id: String,
    pub sender_name: String,
    pub message_id: String,    // 平台消息 id,编排层去重用
    pub text: String,
    pub is_dm: bool,           // 私聊
    pub is_mention: bool,      // 群聊消息是否 @ 了 bot
}

/// 平台传输适配器。每渠道一个实例;`run` 消费 shutdown 令牌,要么运行到停机,
/// 要么因致命错误返回 Err(由编排层折算成 ImChannelRuntime::error 并决定重连)。
/// 构造函数接收组合根注入的 `reqwest::Client` 与已解析的凭证、策略值。
#[async_trait::async_trait]
pub trait ImTransport: Send + Sync {
    fn kind(&self) -> ImChannelKind;
    async fn run(
        &self,
        inbound: InboundSender,
        shutdown: ark_ports::CancelToken,
    ) -> ark_common::Result<()>;
    /// 发送文本;分块到平台上限是实现的职责(编排层给全文)。
    async fn send_text(&self, chat_id: &str, text: &str) -> ark_common::Result<()>;
    /// 凭证校验,返回平台身份描述(如 `@bot_username`);失败返回结构化错误。
    async fn verify(&self) -> ark_common::Result<String>;
}

/// 可选能力:交互式登录编排(目前仅微信二维码登录)。不通用的东西不进主 trait。
/// 登录流由 arkd 装配的独立对象持有(同样注入经治理的 client),不依赖已解析凭证
/// ——凭证恰恰是登录的产物;成功后由编排层把渠道(含凭证)写入 im.json 再触发
/// reconcile 建连。
pub trait ImLoginFlow: Send + Sync {
    /// 进程内短生命周期登录会话(10 分钟过期,同时至多 1 个),不落盘。
    /// `start` 需要网络 IO(取二维码),与 `poll` 同为 async。
    async fn start(&self) -> ark_common::Result<LoginSession>;
    async fn poll(&self, session: &LoginSession) -> ark_common::Result<LoginPoll>;
}
```

- 适配器构造函数接收**已解析的凭证与策略值**(编排层从 `ImStore` 读出后传入),适配器
  不读存储、不看 config 快照;两个例外:注入的 `reqwest::Client` 来自组合根(见上节),
  `ImLoginFlow` 先于任何凭证存在、由组合根单独装配。
- 入站过滤(白名单/@提及/自身消息)的分界:适配器只上报**平台事实**(`is_dm`/`is_mention`),
  策略判定(白名单、`require_mention`、忽略自身)居编排层——策略是需求语义,不该散进
  每个适配器。适配器只负责丢弃无法归一化的消息类型(贴纸、语音等非文本)。
- 去重、命令解析(`/new`)、分块、绑定键归一化是平台无关纯函数,居本 crate 并单测:
  `dedup` 是每渠道有界 LRU(`DEDUP_CAPACITY = 1024`),键为 `(chat_id, message_id)`,
  避免不同会话复用同一平台消息 id 时误杀;`chunk_text` 按平台上限切分,
  `parse_command` 只认 `/new`,其余 `/` 前缀返回 `Command::Unknown` 供编排层提示。
- 平台上限与容量以扩展 trait/函数承载(`pub trait KindLimits
  { fn max_message_chars(&self) -> usize; }` 与 `ensure_channel_capacity`,为
  `ImChannelKind`/渠道集实现于本 crate)——`ark-contract` 只放形状与上限常量
  (`MAX_IM_CHANNELS` 等),不放按 kind 判别的行为(Telegram 4096 等)。

## 平台适配器要点

### Telegram(阶段 2)

零新增依赖:注入的 `reqwest::Client` 直调 `https://api.telegram.org/bot{token}/...`。
`getUpdates` 长轮询(timeout 25s,`allowed_updates=["message"]`),失败指数退避
1.5s→30s 带抖动;一批 update 先归一化并原子写入 durable inbox,server 逐条确认持久
入队或终端丢弃后,适配器才把最终 offset(最后 `update_id + 1`)写入本地游标并恢复拉取。
`sendMessage` HTML parse mode,失败降级纯文本。**v1 仅私聊**:群/supergroup/channel
更新由适配器标记 `is_dm=false`,编排层一律丢弃(不回复);`verify` 即 `getMe`。

### 飞书(阶段 3)

WebSocket 长连接(官方"长连接"事件订阅协议,自实现:`tokio-tungstenite`
(`rustls-tls-webpki-roots`)+ 平台握手/心跳/帧解析;域名按 credentials.domain 区分
feishu/lark)。适配器从 `bot/v3/info` 取得 bot `open_id`;群聊事件只有在
`mentions[].id.open_id` 与该身份相同时才置 `is_mention=true`,提到其他成员或身份未知
均 fail closed。WebSocket echo 只在消息通过 durable admission 后确认。发消息经
`im/v1/messages` REST(`tenant_access_token` 由适配器持有并按过期时间刷新)。降级判定:
长连接协议细节受阻且短期内不可解时,渠道进入 `send_only` 运行态(仅保留 REST 发送、
入站暂不支持)并如实回填需求文档——webhook 入站需要公网回调入口,arkd 是 loopback
边界,不作为降级目标。落地记录:协议自 `@larksuiteoapi/node-sdk` 完整提取(endpoint
拉取 + pbbp2 帧 + ping/pong + 分片重组 + echo ack),降级判定未触发。

### 微信(阶段 4)

iLink bot 协议(`ilink/bot/getupdates` 长轮询 35s + `ilink/bot/sendmessage`,
`ilink_bot_token` 鉴权;协议形状以 `@tencent-weixin/openclaw-weixin` 依赖为参照原生实现,
不引 Node 依赖)。二维码登录经
`ImLoginFlow` 暴露(start/wait 两段,状态机 wait/scanned/expired/confirmed),成功后
`owner_user_id` 随凭证落盘(必填,编排层按它过滤 sender)。一批 `msgs` 先原子写入
durable inbox,server 确认后才推进 `get_updates_buf`。`context_token` 保活仍是适配器
进程内状态,不随 inbox 持久化;重启重放能恢复入站处理,但若主动回复上下文尚缺失,
发送失败按既有 `runtime.last_error` 降级。退出条件:协议逆向不可行或鉴权不稳定时,
微信移出 v1 范围、更新需求非目标并
回填文档。媒体 CDN 上传下载不在范围:非文本消息丢弃并记 `runtime.last_error` 一次
(不刷屏)。

## 编排层的所有权(归属 ark-server,本文只划界)

- 每渠道一个 transport 实例 + 一个编排层持有的 `CancelToken` + 一个 `InboundSender`
  入口;适配器不直接持有裸 `mpsc::Sender`,批量持久化、投递和 30s admission ack 均由
  `InboundSender` 统一负责。编排循环 `select` 入站消息与 `shutdown_receiver()`,每轮 select 顶部检查
  `is_shutting_down()`——两个信号任一到达(或标志位为真)即 cancel 该渠道 token 并等待
  `run` 返回,`Ctrl-C`/drain 这类只置标志位的关闭路径因此同样收敛(对齐 arkd.md 的两条
  shutdown 路径)。渠道增删/凭证变更走 reconcile:按 `id|credentials|policy` 指纹比较,
  变更的渠道 cancel 旧实例、
  重建连接,其余不动。
- 回复路径:先 `SessionStore::subscribe(thread_id, since_seq=当前水位)`(与 SSE 复用
  同一临界区),再 `start_turn`;订阅仅探测终态(事件按 `turn_id` 过滤),最终文本取
  权威投影——**按本回合 `turn_id` 过滤**的最后一条非空 assistant 文本;本回合无
  assistant 文本(失败/中止)时按终态分支发提示,不回退到历史回合文本(投影保证 lag 时
  回复仍完整,turn_id 过滤保证不把上一回合的答案发给用户);超时 ack(120s)与最大等待
  (1800s)是编排层常量。
- `ImChannelRuntime`(连接状态,含飞书 `send_only` 降级态)由编排层维护并进读模型,
  不落盘。

## 存储

生产组合根把 `<root>/IM`（目录名大小写严格）传给 `ImFileStore`，因此文档位于
`<root>/IM/im.json`。本 crate 与存储实现只消费注入的端口或目录，不自行推导全局根。

`im.json`(`{ channels: [ImChannelRecord], bindings: [ImBinding], inbox: [ImInboxEntry] }`)
经 `ark_ports::ImStore` 读写,实现 `ark-store::ImFileStore`:Mutex 串行 + copy-on-write
快照 + tmp/fsync/rename 原子写 + 损坏恢复,同 `ScheduleFileStore` 模式;端口与
`ScheduleStore` 同为**同步**(整份文档一次进程内锁临界区读写,锁内不做网络 IO)。异步
`InboundSender::send_batch` 的批量持久化与 `advance` 的 cursor 提交分别经 `spawn_blocking` 调用
同步 store，保持原有顺序与 admission ack 语义；JoinError 按取消/panic 映射为
`RuntimeUnavailable`/`StoreFailure` 并携带操作与渠道上下文。这消除同步文件重写对 async executor
的结构性阻塞风险，不表示已实测生产瓶颈。
游标(telegram update offset 等)不进 wire 模型、GET 不回传,也不再有节流窗口:
适配器的一批消息先在**一次原子 rewrite** 中全部落为 `queued=false` 的 inbox 记录;
server 对每条记录再次原子写入 `queued=true` 与最终 cursor,记录继续保留;只有回合
到达终态才 `ack_inbound` 删除。崩溃后 pending/queued 记录都在新 transport 启动前重放,
因此 cursor 前进不会绕过仍在磁盘上的消息。这是至少一次语义:处理完成但 ack 前崩溃
可能重复执行回合。绑定与渠道(含凭证)同文档,因为二者同域变更、且渠道删除需要原子
级联删除其绑定——这也是它独立于 `config.json` 的理由。凭证明文落盘是 provider
`api_key` 同款 v1 简化;读接口换 `ImCredentialsView`、web 只写不回读。

## 验证

- `cargo test -p ark-im --lib`:归一化、`(chat_id, message_id)` 去重 LRU 有界、
  飞书 bot `open_id` 提及判定、批次持久化失败不部分落盘、`chunk_text` 平台上限、命令解析、
  凭证按 kind 必填校验、容量校验。
- `cargo test -p ark-server --lib`:假 `ImTransport` 下入站→白名单/去重/队列→绑定→
  `start_turn`→终态→按 turn_id 投影取文本→`send_text` 回发全链路;渠道 CRUD/上限/
  verify 路由;reconcile 增删重建;pending/queued 重启重放、处理中不 ack、同一 inbox id
  幂等重放、同一活跃平台消息的不同 durable id 去重、transport 失败先 drain 已持久
  delivery、live 队列满终端丢弃而 replay 队列满保留并等待容量后继续处理;409 有界重试;shutdown 收敛。
- `cargo clippy -p ark-im -p ark-server --all-targets -D warnings`;契约一致性由
  ark-contract 的 ts_export 测试承接。
