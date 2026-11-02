//! Inbound pipeline for IM messages (requirements/im-channels.md «入站管线»).
//!
//! Split from [`super::im`] because it is the *message* half of the service:
//! policy filtering, the per-conversation serial queues, the turn drive and the
//! reply waiter. The lifecycle half (which channels run at all) stays in
//! `im.rs`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ark_common::text::truncate_utf8;
use ark_common::{ArkErrorCode, ThreadId};
use ark_contract::{
    ImChannel, ImChannelId, ImChannelKind, ImChannelRuntime, ImCredentials, ThreadRuntimeSettings,
};
use ark_im::{
    channel_autonomy, parse_command, truncate_inbound, Command, ImTransport, InboundMessage,
};
use ark_ports::binding_now;
use tokio::sync::mpsc;

use crate::state::{AppState, TurnSource};

use super::im::{CONVERSATION_QUEUE_CAP, IM_ACK_SECS, IM_BUSY_RETRY_SECS, IM_MAX_WAIT_SECS};

/// Poll cadence for the terminal-state fallback while a reply waiter listens.
const TERMINAL_POLL: Duration = Duration::from_secs(2);
/// Title budget for threads created from an IM first message.
const THREAD_TITLE_MAX_BYTES: usize = 40;

/// Per-channel state the pipeline needs while a supervisor runs.
pub struct ChannelContext {
    pub channel: ImChannel,
    pub transport: Arc<dyn ImTransport>,
    pub runtime: Arc<Mutex<ImChannelRuntime>>,
}

/// Outcome of the terminal wait, telling the reply step which branch to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnOutcome {
    Completed,
    Failed,
    Aborted,
}

/// Serial per-conversation dispatch (requirements «派发»).
///
/// One agent turn can run for minutes; queues keep a chatty conversation
/// ordered without blocking the channel's other chats. Depth is bounded — an
/// overflowing chat gets a busy reply instead of unbounded memory.
pub struct ConversationQueues(Mutex<HashMap<(ImChannelId, String), Arc<Conversation>>>);

impl Default for ConversationQueues {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct Conversation {
    queue: Mutex<VecDeque<InboundMessage>>,
    working: AtomicBool,
}

pub enum Enqueue {
    /// Nobody was working: the caller must start the drain loop.
    Started,
    /// Queued behind the running turn.
    Queued,
    /// Queue full: the caller answers with a busy reply.
    Full,
}

impl ConversationQueues {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(ImChannelId, String), Arc<Conversation>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn enqueue(&self, key: (ImChannelId, String), message: InboundMessage) -> Enqueue {
        let conversation = self.lock().entry(key).or_default().clone();
        {
            let mut queue = conversation
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if queue.len() >= CONVERSATION_QUEUE_CAP {
                return Enqueue::Full;
            }
            queue.push_back(message);
        }
        if conversation.working.swap(true, Ordering::AcqRel) {
            Enqueue::Queued
        } else {
            Enqueue::Started
        }
    }

    /// Pop the next message, or clear the "working" flag when the queue ran
    /// dry. The re-check under the lock is what makes this race-free against a
    /// concurrent enqueue: a message pushed after the empty check but before
    /// the flag clears would be lost without it.
    pub fn next(&self, key: &(ImChannelId, String)) -> Option<InboundMessage> {
        let conversation = self.lock().get(key).cloned()?;
        let mut queue = conversation
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match queue.pop_front() {
            Some(message) => Some(message),
            None => {
                conversation.working.store(false, Ordering::Release);
                None
            }
        }
    }
}

impl super::im::ImService {
    /// First stage of the pipeline: filter, dedup, answer commands, then hand
    /// the message to its conversation queue.
    pub async fn accept_inbound(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: InboundMessage,
    ) {
        self.record_inbound(&ctx.channel.id);
        if ctx.transport.is_self(&message.sender_id) {
            return;
        }
        if message.message_id.is_empty() || self.is_duplicate(&ctx.channel.id, &message.message_id)
        {
            return;
        }
        if !message.is_dm {
            // Telegram v1 is private-chat only (requirements «平台适配器要点»);
            // the other platforms' groups answer to the mention gate.
            if ctx.channel.kind == ImChannelKind::Telegram {
                return;
            }
            if ctx.channel.policy.require_mention && !message.is_mention {
                return;
            }
        }
        // WeChat is 1:1 by protocol; the owner binding is the second gate.
        if let Some(ImCredentials::Weixin { owner_user_id, .. }) = &ctx.channel.credentials {
            if message.sender_id != *owner_user_id {
                return;
            }
        }
        let text = message.text.trim();
        if text.is_empty() {
            return;
        }
        // Whitelist. Empty list = default-closed: answer a *new* chat with an
        // onboarding hint (it needs the chat id to self-serve), but never open
        // a turn for it. A non-empty list silently drops outsiders — sending
        // anything to an unknown third party is worse than silence.
        let allowed = &ctx.channel.policy.allowed_chat_ids;
        if allowed.is_empty() {
            let bound = self
                .store()
                .get_binding(&ctx.channel.id, &message.chat_id)
                .ok()
                .flatten()
                .is_some();
            if !bound {
                let hint = format!(
                    "本渠道尚未配置白名单，未开始处理会话。请在 ark 设置页将该 chat id 加入渠道白名单：{chat}\n\nThis channel has no whitelist yet; add this chat id in the ark settings page: {chat}",
                    chat = message.chat_id
                );
                self.send_reply(&ctx.channel.id, ctx, &message.chat_id, hint)
                    .await;
                return;
            }
        } else if !allowed.iter().any(|id| id == &message.chat_id) {
            return;
        }

        match parse_command(text) {
            Command::New => {
                self.handle_new_command(state, ctx, &message).await;
                return;
            }
            Command::Unknown(name) => {
                self.send_reply(
                    &ctx.channel.id,
                    ctx,
                    &message.chat_id,
                    format!("{name} 不是可用命令；当前支持：/new"),
                )
                .await;
                return;
            }
            Command::None => {}
        }

        let key = (ctx.channel.id.clone(), message.chat_id.clone());
        match self.conversations.enqueue(key.clone(), message) {
            Enqueue::Full => {
                self.send_reply(
                    &ctx.channel.id,
                    ctx,
                    &key.1,
                    "上一条消息仍在处理中，队列已满，请稍后再发。",
                )
                .await;
            }
            Enqueue::Queued => {}
            Enqueue::Started => {
                let service = Arc::clone(self);
                let ctx = Arc::clone(ctx);
                let state = state.clone();
                let chat_id = key.1;
                tokio::spawn(async move {
                    service.drain_conversation(&state, &ctx, chat_id).await;
                });
            }
        }
    }

    /// Drain one conversation's queue serially.
    async fn drain_conversation(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        chat_id: String,
    ) {
        let key = (ctx.channel.id.clone(), chat_id);
        while let Some(message) = self.conversations.next(&key) {
            self.process_message(state, ctx, message).await;
        }
    }

    /// `/new`: drop the binding; the next message creates a fresh thread.
    async fn handle_new_command(
        self: &Arc<Self>,
        _state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: &InboundMessage,
    ) {
        let result = self
            .store()
            .delete_binding(&ctx.channel.id, &message.chat_id);
        let reply = match result {
            Ok(()) => "已重置会话，下一条消息将开启新会话。 (/new)".to_string(),
            Err(err) => format!("重置会话失败：{err}"),
        };
        self.send_reply(&ctx.channel.id, ctx, &message.chat_id, reply)
            .await;
    }

    /// Second stage: resolve the thread, run the turn, deliver the reply.
    async fn process_message(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: InboundMessage,
    ) {
        let Some((thread_id, since_seq)) = self.resolve_thread(state, ctx, &message).await else {
            return;
        };
        // Subscribe before the turn starts so the terminal event cannot slip
        // between registration and subscription (requirements «回复路径»).
        // The watermark is the thread's `last_seq`, so the backlog is at most
        // this turn's own events.
        let ticket = match state.store.subscribe(thread_id, since_seq).await {
            Ok(ticket) => ticket,
            Err(err) => {
                self.send_reply(
                    &ctx.channel.id,
                    ctx,
                    &message.chat_id,
                    format!("无法订阅会话事件：{err}"),
                )
                .await;
                return;
            }
        };
        let text = truncate_inbound(&message.text);
        let turn = match self
            .start_with_retry(state, ctx, &message, thread_id, text)
            .await
        {
            Some(turn_id) => turn_id,
            None => return, // retries exhausted or a start error was replied
        };
        self.wait_and_reply(state, ctx, &message, thread_id, turn, ticket.live)
            .await;
    }

    /// The bound thread, creating (or recreating after a 404) as needed.
    async fn resolve_thread(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: &InboundMessage,
    ) -> Option<(ThreadId, u64)> {
        let existing = match self.store().get_binding(&ctx.channel.id, &message.chat_id) {
            Ok(Some(binding)) => Some(binding.thread_id),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err.message, "cannot read im binding");
                return None;
            }
        };
        if let Some(thread_id) = existing {
            match state.store.get_thread(thread_id).await {
                Ok(_) => {
                    let since = state.store.highest_seq(thread_id).await.unwrap_or(0);
                    return Some((thread_id, since));
                }
                // The thread was deleted out from under the binding: rebind.
                Err(_) => {
                    let _ = self
                        .store()
                        .delete_binding(&ctx.channel.id, &message.chat_id);
                }
            }
        }
        let title = truncate_utf8(message.text.trim(), THREAD_TITLE_MAX_BYTES);
        let created = match crate::service::threads::create_thread(state, title).await {
            Ok(summary) => summary,
            Err(err) => {
                self.send_reply(
                    &ctx.channel.id,
                    ctx,
                    &message.chat_id,
                    format!("创建会话失败：{err}"),
                )
                .await;
                return None;
            }
        };
        // Unattended by default: nobody at the desktop will answer a gate.
        let runtime = ThreadRuntimeSettings {
            workspace: ctx.channel.workspace.clone(),
            provider_id: ctx.channel.provider_id.clone(),
            model: ctx.channel.model.clone(),
            autonomy: Some(channel_autonomy(&ctx.channel)),
            reasoning_effort: None,
        };
        if let Err(err) = state
            .store
            .set_thread_runtime(created.thread_id, runtime)
            .await
        {
            tracing::warn!(error = %err.message, "cannot apply im thread runtime overrides");
        }
        let binding = binding_now(
            ctx.channel.id.clone(),
            message.chat_id.clone(),
            created.thread_id,
        );
        if let Err(err) = self.store().set_binding(binding) {
            tracing::warn!(error = %err.message, "cannot persist im binding");
        }
        Some((created.thread_id, created.last_seq.unwrap_or(0)))
    }

    /// Start the turn, retrying a bounded number of times when the bound
    /// thread is busy with someone else's turn (desktop user, review route).
    async fn start_with_retry(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: &InboundMessage,
        thread_id: ThreadId,
        text: String,
    ) -> Option<ark_common::TurnId> {
        let source = TurnSource::Im {
            channel_id: ctx.channel.id.to_string(),
            chat_id: message.chat_id.clone(),
            text: text.clone(),
        };
        let mut retries = 0usize;
        loop {
            match crate::service::start_turn_as(
                state,
                thread_id,
                source.clone(),
                text.clone(),
                Vec::new(),
            )
            .await
            {
                Ok(summary) => return Some(summary.turn_id),
                Err(err) if err.code == ArkErrorCode::Conflict => {
                    retries += 1;
                    if retries > super::im::IM_BUSY_RETRIES {
                        self.send_reply(
                            &ctx.channel.id,
                            ctx,
                            &message.chat_id,
                            "会话正在处理桌面端发起的回合，请稍后再发。",
                        )
                        .await;
                        return None;
                    }
                    if state.is_shutting_down() {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_secs(IM_BUSY_RETRY_SECS)).await;
                }
                Err(err) => {
                    self.send_reply(
                        &ctx.channel.id,
                        ctx,
                        &message.chat_id,
                        format!("回合启动失败：{err}"),
                    )
                    .await;
                    return None;
                }
            }
        }
    }

    /// Detect the turn's terminal event, then answer from the authoritative
    /// projection (never from accumulated deltas — a lagging live channel must
    /// not truncate a reply).
    async fn wait_and_reply(
        self: &Arc<Self>,
        state: &AppState,
        ctx: &Arc<ChannelContext>,
        message: &InboundMessage,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        mut live: mpsc::Receiver<ark_contract::RuntimeEvent>,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(IM_MAX_WAIT_SECS);
        let ack_at = tokio::time::Instant::now() + Duration::from_secs(IM_ACK_SECS);
        let mut acked = false;
        let outcome = loop {
            if state.is_shutting_down() {
                return;
            }
            // Registry poll first: the supervisor writes terminal states on
            // every path, so this alone is a complete (if coarser) detector.
            if let Some(summary) = state.turns.get(turn_id) {
                if summary.ended_at.is_some() {
                    break outcome_from_state(&summary.state, summary.error.as_deref());
                }
            }
            let poll = tokio::time::sleep(TERMINAL_POLL);
            tokio::pin!(poll);
            tokio::select! {
                event = live.recv() => {
                    match event {
                        Some(event) => {
                            if let Some(outcome) = terminal_outcome(&event, turn_id) {
                                break outcome;
                            }
                        }
                        // Live channel disconnected (slow consumer): the
                        // registry poll above remains the detector.
                        None => {
                            tokio::time::sleep(TERMINAL_POLL).await;
                        }
                    }
                }
                _ = &mut poll => {}
                _ = tokio::time::sleep_until(ack_at), if !acked => {
                    acked = true;
                    self.send_reply(
                        &ctx.channel.id,
                        ctx,
                        &message.chat_id,
                        "收到，正在处理…（完成后自动回复）".to_string(),
                    )
                    .await;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.send_reply(
                        &ctx.channel.id,
                        ctx,
                        &message.chat_id,
                        "处理超时；回合仍在后台运行，结果保留在会话线程中。",
                    )
                    .await;
                    return;
                }
            }
        };
        match outcome {
            TurnOutcome::Completed => {
                let text = final_reply(state, thread_id, turn_id).await;
                if let Some(text) = text {
                    self.send_reply(&ctx.channel.id, ctx, &message.chat_id, text)
                        .await;
                }
            }
            TurnOutcome::Failed => {
                let detail = state
                    .turns
                    .get(turn_id)
                    .and_then(|summary| summary.error.clone())
                    .unwrap_or_else(|| "模型回合失败".to_string());
                self.send_reply(
                    &ctx.channel.id,
                    ctx,
                    &message.chat_id,
                    format!("处理失败：{}", truncate_utf8(&detail, 300)),
                )
                .await;
            }
            TurnOutcome::Aborted => {
                self.send_reply(&ctx.channel.id, ctx, &message.chat_id, "回合已中止。")
                    .await;
            }
        }
    }

    /// Deliver one reply; failures degrade to the runtime's `last_error`,
    /// never to a panic or a retry storm.
    pub async fn send_reply(
        self: &Arc<Self>,
        channel_id: &ImChannelId,
        ctx: &Arc<ChannelContext>,
        chat_id: &str,
        text: impl Into<String>,
    ) {
        let text = text.into();
        if let Err(err) = ctx.transport.send_text(chat_id, &text).await {
            tracing::warn!(channel = %channel_id, error = %err.message, "im reply send failed");
            let mut cell = ctx.runtime.lock().unwrap_or_else(PoisonError::into_inner);
            cell.last_error = Some(err.message);
        }
    }
}

/// Map a registry summary onto the reply branch.
fn outcome_from_state(state: &str, error: Option<&str>) -> TurnOutcome {
    // The registry records the same words the events use.
    match state {
        "failed" => TurnOutcome::Failed,
        "aborted" => TurnOutcome::Aborted,
        _ => {
            let _ = error;
            TurnOutcome::Completed
        }
    }
}

/// Terminal events carry the turn id; everything else is ignored.
fn terminal_outcome(
    event: &ark_contract::RuntimeEvent,
    turn_id: ark_common::TurnId,
) -> Option<TurnOutcome> {
    use ark_contract::RuntimeEvent;
    match event {
        RuntimeEvent::TurnCompleted { turn_id: id, .. } if *id == turn_id => {
            Some(TurnOutcome::Completed)
        }
        RuntimeEvent::TurnFailed { turn_id: id, .. } if *id == turn_id => Some(TurnOutcome::Failed),
        RuntimeEvent::TurnAborted { turn_id: id, .. } if *id == turn_id => {
            Some(TurnOutcome::Aborted)
        }
        _ => None,
    }
}

/// Last non-empty assistant text of *this* turn, from the thread's own items.
///
/// `None` means the turn completed without assistant text; the caller stays
/// silent instead of falling back to a previous turn's answer.
async fn final_reply(
    state: &AppState,
    thread_id: ThreadId,
    turn_id: ark_common::TurnId,
) -> Option<String> {
    let items = state.store.read_items(thread_id).await.ok()?;
    items
        .iter()
        .rev()
        .filter(|item| {
            item.turn_id == turn_id && item.kind == ark_contract::ItemKind::AssistantMessage
        })
        .find_map(|item| {
            item.text
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
        })
        .map(|text| text.to_string())
}
