//! Platform-neutral IM domain: normalized message shape, payload
//! normalization, dedup, chunking, command parsing and capacity.
//!
//! Everything here is a pure function or a plain data structure so the
//! orchestration in `ark-server` cannot grow a second, diverging copy of the
//! same rule (the `ark-schedule` precedent). Policy *decisions* (whitelist
//! branches, mention gating) stay in the orchestration because they need the
//! binding table; this module only repairs and validates shapes.

use ark_common::text::truncate_utf8;
use ark_common::{ArkError, Result};
use ark_contract::{
    CreateImChannelRequest, ImChannel, ImChannelId, ImChannelKind, ImChannelPolicy, ImCredentials,
    UpdateImChannelRequest, DEFAULT_IM_AUTONOMY, IM_TITLE_MAX_BYTES, MAX_IM_CHANNELS,
};
use chrono::Utc;

pub use ark_ports::InboundMessage;

/// Bounded dedup set per channel (docs/requirements/im-channels.md «入站管线»).
///
/// Memory-only by design: cross-restart replay is bounded by the persisted
/// channel cursor instead, so growing this to disk would only duplicate what
/// the cursor already guarantees.
pub const DEDUP_CAPACITY: usize = 1024;

/// Inbound text is clamped to the same wire limit a user message has, so an
/// oversized paste degrades to a truncated turn instead of a rejected chat.
pub fn truncate_inbound(text: &str) -> String {
    truncate_utf8(text, ark_contract::MAX_TURN_TEXT_BYTES).to_string()
}

/// Bounded LRU of already-seen `(chat_id, message_id)` pairs.
#[derive(Debug, Default)]
pub struct DedupSet {
    seen: std::collections::HashSet<(String, String)>,
    order: std::collections::VecDeque<(String, String)>,
}

impl DedupSet {
    pub fn contains(&self, chat_id: &str, message_id: &str) -> bool {
        self.seen
            .contains(&(chat_id.to_string(), message_id.to_string()))
    }

    /// `true` when the pair was not seen before (and is now recorded).
    /// Oldest pairs are evicted once the set reaches [`DEDUP_CAPACITY`].
    pub fn insert(&mut self, chat_id: &str, message_id: &str) -> bool {
        let key = (chat_id.to_string(), message_id.to_string());
        if self.seen.contains(&key) {
            return false;
        }
        if self.order.len() >= DEDUP_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(key.clone());
        self.order.push_back(key);
        true
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// A recognized inbound command. Only `/new` is in scope (the
/// command set beyond it is a documented non-goal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Rebind the chat to a fresh thread.
    New,
    /// A `/`-prefixed message the runtime does not implement; the orchestration
    /// answers with a short hint instead of treating it as a prompt.
    Unknown(String),
    /// Plain prompt text.
    None,
}

/// Classify one inbound text.
pub fn parse_command(text: &str) -> Command {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Command::None;
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    match parts.next() {
        Some("new") => Command::New,
        Some(name) => Command::Unknown(format!("/{name}")),
        None => Command::None,
    }
}

/// Platform per-message character ceilings, as a trait on the contract enum.
///
/// The constants live here (not in `ark-contract`) because they are adapter
/// behaviour keyed by kind, and the contract crate carries shapes and shared
/// constants only.
pub trait KindLimits {
    /// Conservative per-message text budget used by [`chunk_text`].
    fn max_message_chars(&self) -> usize;
}

impl KindLimits for ImChannelKind {
    fn max_message_chars(&self) -> usize {
        match self {
            // Telegram's documented hard limit; chunks stay comfortably below
            // it because the limit counts UTF-16 code units, not chars.
            ImChannelKind::Telegram => 4096,
            // Feishu text messages tolerate far more, but a single enormous
            // bubble is unreadable in chat; keep chat-sized pieces.
            ImChannelKind::Feishu => 4000,
            // WeChat iLink bot messages have no documented large-text path.
            ImChannelKind::Weixin => 2048,
        }
    }
}

/// Length in UTF-16 code units, which is what Telegram's message limit counts.
/// A non-BMP scalar (emoji, CJK extension B) occupies two units.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Split at the largest char boundary whose UTF-16 length fits `max_units`.
/// Always consumes at least one char so a wide scalar cannot stall the caller.
fn split_at_utf16(text: &str, max_units: usize) -> (&str, &str) {
    let mut used = 0;
    let mut byte_end = 0;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > max_units && byte_end > 0 {
            break;
        }
        used += width;
        byte_end += ch.len_utf8();
        if used >= max_units {
            break;
        }
    }
    text.split_at(byte_end)
}

/// Split a reply into platform-sized pieces on paragraph/line boundaries,
/// falling back to a hard cut when one line exceeds the limit. All budgets are
/// measured in UTF-16 code units so a message full of surrogate pairs still
/// fits Telegram's documented hard limit (a plain `chars().count()` did not).
pub fn chunk_text(kind: ImChannelKind, text: &str) -> Vec<String> {
    let limit = kind.max_message_chars();
    if utf16_len(text) <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if utf16_len(line) > limit {
            // One overlong line: hard-cut it on UTF-16-safe char boundaries.
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let mut rest = line;
            while utf16_len(rest) > limit {
                let (head, tail) = split_at_utf16(rest, limit);
                chunks.push(head.to_string());
                rest = tail;
            }
            current = rest.to_string();
            continue;
        }
        if utf16_len(&current) + utf16_len(line) > limit {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Reject creation beyond the channel cap.
///
/// Same shape as `ark-schedule::ensure_capacity`: the caller reads the live
/// table, then calls this, then inserts — the window is acceptable for a
/// single-user desktop runtime.
pub fn ensure_channel_capacity(existing: usize) -> Result<()> {
    if existing >= MAX_IM_CHANNELS {
        return Err(validation_with_detail(
            format!("channel limit reached ({MAX_IM_CHANNELS}); delete a channel first"),
            "im_channel_limit",
        ));
    }
    Ok(())
}

/// Every secret field this credential set requires must be present.
pub fn validate_credentials(credentials: Option<&ImCredentials>) -> Result<&ImCredentials> {
    let credentials = credentials.ok_or_else(|| {
        validation_with_detail("credentials are required", "im_credentials_missing")
    })?;
    if !credentials.secrets_present() {
        return Err(validation_with_detail(
            "credentials are incomplete: every secret field must be non-empty",
            "im_credentials_missing",
        ));
    }
    Ok(credentials)
}

/// A validation error that names a machine-readable detail, the way
/// `ark-schedule` tags `schedule_limit`.
fn validation_with_detail(message: impl Into<String>, detail: &str) -> ArkError {
    let mut err = ArkError::validation(message);
    err.detail = Some(detail.to_string());
    err
}

fn normalize_title(title: Option<String>, fallback: &str) -> String {
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    truncate_utf8(&title, IM_TITLE_MAX_BYTES).to_string()
}

fn normalize_policy(mut policy: ImChannelPolicy) -> ImChannelPolicy {
    // Trim and deduplicate chat ids: a stray space or a repeated entry must not
    // make the whitelist look bigger than it is.
    policy.allowed_chat_ids = policy
        .allowed_chat_ids
        .iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    policy.allowed_chat_ids.sort();
    policy
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Build a stored channel from a create request (requirements «数据模型»).
///
/// Repair-first like `ark-schedule::prepare_create`, with the one documented
/// hard error: missing/incomplete credentials are a structured validation
/// error instead of a channel that opens a guaranteed-failing connection.
pub fn prepare_create(request: CreateImChannelRequest) -> Result<ImChannel> {
    validate_credentials(request.credentials.as_ref())?;
    let now = Utc::now();
    Ok(ImChannel {
        id: ImChannelId::new(),
        kind: request.kind,
        title: normalize_title(request.title, "IM channel"),
        // A create request never enables implicitly: the caller decides. An
        // absent flag reads as disabled so a malformed payload cannot open a
        // connection nobody asked for.
        enabled: request.enabled.unwrap_or(false),
        credentials: request.credentials,
        policy: normalize_policy(request.policy.unwrap_or_default()),
        workspace: trim_optional(request.workspace),
        model: trim_optional(request.model),
        provider_id: trim_optional(request.provider_id),
        autonomy: trim_optional(request.autonomy),
        created_at: now,
        updated_at: now,
    })
}

/// Merge an update request onto a stored channel.
///
/// Every present field replaces, absent fields keep. An *empty-string* secret
/// field keeps the stored secret (provider `api_key` rule): edit forms that do
/// not re-display secrets would otherwise wipe them on save. The stored
/// credentials win whenever the incoming variant carries an empty secret.
pub fn apply_update(current: &ImChannel, request: UpdateImChannelRequest) -> Result<ImChannel> {
    let mut next = current.clone();
    if let Some(title) = request.title {
        next.title = normalize_title(Some(title), "IM channel");
    }
    if let Some(enabled) = request.enabled {
        next.enabled = enabled;
    }
    if let Some(credentials) = request.credentials {
        if credentials.secrets_present() {
            next.credentials = Some(credentials);
        } else {
            // Merge the non-secret parts of an incomplete credential set onto
            // the stored one; without stored credentials an incomplete set is
            // still an error.
            let stored = current.credentials.as_ref().ok_or_else(|| {
                validation_with_detail("credentials are required", "im_credentials_missing")
            })?;
            next.credentials = Some(merge_credentials(stored, &credentials));
        }
    }
    if let Some(policy) = request.policy {
        next.policy = normalize_policy(policy);
    }
    // Optional text fields: empty string clears, absent keeps (schedules rule).
    if request.workspace.is_some() {
        next.workspace = trim_optional(request.workspace);
    }
    if request.model.is_some() {
        next.model = trim_optional(request.model);
    }
    if request.provider_id.is_some() {
        next.provider_id = trim_optional(request.provider_id);
    }
    if request.autonomy.is_some() {
        next.autonomy = trim_optional(request.autonomy).or(Some(DEFAULT_IM_AUTONOMY.to_string()));
    }
    next.updated_at = Utc::now();
    validate_credentials(next.credentials.as_ref())?;
    Ok(next)
}

/// Overlay non-secret fields of `incoming` onto `stored`, keeping stored
/// secrets when the incoming secret fields are empty.
fn merge_credentials(stored: &ImCredentials, incoming: &ImCredentials) -> ImCredentials {
    match (stored, incoming) {
        (
            ImCredentials::Telegram {
                bot_token,
                bot_username,
            },
            ImCredentials::Telegram {
                bot_token: new_token,
                bot_username: new_username,
            },
        ) => ImCredentials::Telegram {
            bot_token: non_empty_or(new_token, bot_token),
            bot_username: new_username.clone().or_else(|| bot_username.clone()),
        },
        (
            ImCredentials::Feishu {
                app_id, app_secret, ..
            },
            ImCredentials::Feishu {
                app_id: new_id,
                app_secret: new_secret,
                domain: new_domain,
            },
        ) => ImCredentials::Feishu {
            app_id: non_empty_or(new_id, app_id),
            app_secret: non_empty_or(new_secret, app_secret),
            domain: *new_domain,
        },
        (
            ImCredentials::Weixin {
                bot_token,
                base_url,
                owner_user_id,
            },
            ImCredentials::Weixin {
                bot_token: new_token,
                base_url: new_base,
                owner_user_id: new_owner,
            },
        ) => ImCredentials::Weixin {
            bot_token: non_empty_or(new_token, bot_token),
            base_url: new_base.clone().or_else(|| base_url.clone()),
            owner_user_id: non_empty_or(new_owner, owner_user_id),
        },
        // A kind switch with an incomplete credential set is a caller bug; the
        // stored credentials stay untouched.
        _ => stored.clone(),
    }
}

fn non_empty_or(incoming: &str, stored: &str) -> String {
    if incoming.trim().is_empty() {
        stored.to_string()
    } else {
        incoming.to_string()
    }
}

/// The autonomy a channel's turns run with.
pub fn channel_autonomy(channel: &ImChannel) -> String {
    channel
        .autonomy
        .clone()
        .unwrap_or_else(|| DEFAULT_IM_AUTONOMY.to_string())
}

/// Build the transport for one channel's kind.
///
/// The `client` is injected by the composition root (HTTP governance); the
/// `cursor` resumes the platform stream where the persisted record left off.
pub fn build_transport(
    client: reqwest::Client,
    kind: ImChannelKind,
    credentials: &ImCredentials,
    cursor: Option<String>,
) -> Result<std::sync::Arc<dyn crate::ImTransport>> {
    match (kind, credentials) {
        (
            ImChannelKind::Telegram,
            ImCredentials::Telegram {
                bot_token,
                bot_username,
            },
        ) => Ok(std::sync::Arc::new(
            crate::telegram::TelegramTransport::new(
                client,
                bot_token.clone(),
                bot_username.clone(),
                cursor,
            ),
        )),
        (
            ImChannelKind::Feishu,
            ImCredentials::Feishu {
                app_id,
                app_secret,
                domain,
            },
        ) => Ok(std::sync::Arc::new(crate::feishu::FeishuTransport::new(
            client,
            app_id.clone(),
            app_secret.clone(),
            *domain,
        ))),
        (
            ImChannelKind::Weixin,
            ImCredentials::Weixin {
                bot_token,
                base_url,
                owner_user_id,
            },
        ) => Ok(std::sync::Arc::new(crate::weixin::WeixinTransport::new(
            client,
            bot_token.clone(),
            base_url.clone(),
            owner_user_id.clone(),
            cursor,
        ))),
        // A credential kind that does not match the channel kind is a stored
        // document inconsistency, not a runtime condition.
        _ => Err(ArkError::validation(
            "credentials kind does not match the channel kind",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ArkErrorCode;
    use ark_contract::FeishuDomain;

    fn telegram_credentials() -> ImCredentials {
        ImCredentials::Telegram {
            bot_token: "tok".into(),
            bot_username: None,
        }
    }

    #[test]
    fn dedup_set_is_bounded_and_lru() {
        let mut set = DedupSet::default();
        for i in 0..DEDUP_CAPACITY {
            assert!(set.insert("100", &i.to_string()));
        }
        // A retained pair is a duplicate; a new pair evicts the oldest, which
        // then reads as new again.
        assert!(!set.insert("100", "5"));
        assert!(set.insert("100", "9999"));
        assert!(set.insert("100", "0"));
        assert_eq!(set.len(), DEDUP_CAPACITY);

        let mut scoped = DedupSet::default();
        assert!(scoped.insert("100", "7"));
        assert!(!scoped.insert("100", "7"));
        assert!(scoped.insert("200", "7"));
    }

    #[test]
    fn command_parsing_matches_the_scope() {
        assert_eq!(parse_command("/new"), Command::New);
        assert_eq!(parse_command(" /new rest"), Command::New);
        assert_eq!(parse_command("/list"), Command::Unknown("/list".into()));
        assert_eq!(parse_command("hello"), Command::None);
        assert_eq!(parse_command("/"), Command::Unknown("/".into()));
    }

    #[test]
    fn chunking_respects_the_platform_limit() {
        let text = "a".repeat(10_000);
        for chunk in chunk_text(ImChannelKind::Telegram, &text) {
            assert!(chunk.chars().count() <= 4096);
        }
        assert_eq!(chunk_text(ImChannelKind::Telegram, "short"), vec!["short"]);
        // Paragraph-boundary chunking keeps lines intact when they fit.
        let lines = format!("{}\n{}", "b".repeat(2000), "c".repeat(2000));
        assert_eq!(chunk_text(ImChannelKind::Telegram, &lines).len(), 1);
    }

    #[test]
    fn chunking_counts_utf16_units_not_chars() {
        // Telegram's 4096 limit counts UTF-16 code units. A naive
        // `chars().count()` chunk of 4096 emoji is 8192 units and is rejected.
        let emoji = "\u{1F600}".repeat(4096);
        let chunks = chunk_text(ImChannelKind::Telegram, &emoji);
        assert!(
            chunks.len() > 1,
            "a surrogate-pair-only message must be split"
        );
        let mut rebuilt = String::new();
        for chunk in &chunks {
            assert!(
                chunk.chars().map(char::len_utf16).sum::<usize>() <= 4096,
                "chunk exceeds the UTF-16 budget"
            );
            rebuilt.push_str(chunk);
        }
        assert_eq!(rebuilt, emoji, "chunking must not lose or reorder text");

        // BMP CJK text behaves like the old char count.
        let cjk = "中".repeat(3000);
        for chunk in chunk_text(ImChannelKind::Telegram, &cjk) {
            assert!(chunk.chars().map(char::len_utf16).sum::<usize>() <= 4096);
        }
    }

    #[test]
    fn create_requires_credentials_and_rejects_incomplete_secrets() {
        let request = CreateImChannelRequest {
            kind: ImChannelKind::Telegram,
            title: Some("  my bot  ".into()),
            enabled: Some(true),
            credentials: None,
            ..Default::default()
        };
        assert!(prepare_create(request).is_err());

        let request = CreateImChannelRequest {
            kind: ImChannelKind::Telegram,
            title: Some("  my bot  ".into()),
            enabled: None,
            credentials: Some(ImCredentials::Telegram {
                bot_token: "  ".into(),
                bot_username: None,
            }),
            ..Default::default()
        };
        assert!(prepare_create(request).is_err());
    }

    #[test]
    fn create_normalizes_title_policy_and_defaults() {
        let channel = prepare_create(CreateImChannelRequest {
            kind: ImChannelKind::Feishu,
            title: Some("  my bot  ".into()),
            enabled: None,
            credentials: Some(ImCredentials::Feishu {
                app_id: "a".into(),
                app_secret: "s".into(),
                domain: FeishuDomain::Feishu,
            }),
            policy: Some(ImChannelPolicy {
                allowed_chat_ids: vec!["b".into(), " a ".into(), "a".into()],
                require_mention: false,
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(channel.title, "my bot");
        assert!(!channel.enabled);
        assert_eq!(
            channel.policy.allowed_chat_ids,
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(!channel.policy.require_mention);
        // The store assigns the final id; the create path already pre-assigns
        // one so an upsert caller cannot collide on an empty key.
        assert!(channel.id.0.starts_with("im_"));
    }

    #[test]
    fn update_keeps_secrets_when_the_form_sends_empty_ones() {
        let mut channel = prepare_create(CreateImChannelRequest {
            kind: ImChannelKind::Telegram,
            title: Some("bot".into()),
            enabled: Some(true),
            credentials: Some(telegram_credentials()),
            ..Default::default()
        })
        .unwrap();
        channel.id = ImChannelId("im_1".into());

        let next = apply_update(
            &channel,
            UpdateImChannelRequest {
                title: Some("renamed".into()),
                credentials: Some(ImCredentials::Telegram {
                    bot_token: "  ".into(),
                    bot_username: Some("new_name".into()),
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(next.title, "renamed");
        match next.credentials {
            Some(ImCredentials::Telegram {
                bot_token,
                bot_username,
            }) => {
                assert_eq!(bot_token, "tok"); // stored secret survived
                assert_eq!(bot_username.as_deref(), Some("new_name"));
            }
            other => panic!("unexpected credentials: {other:?}"),
        }
    }

    #[test]
    fn update_empty_string_clears_optional_text_fields() {
        let channel = prepare_create(CreateImChannelRequest {
            kind: ImChannelKind::Telegram,
            title: Some("bot".into()),
            credentials: Some(telegram_credentials()),
            workspace: Some("D:/work".into()),
            ..Default::default()
        })
        .unwrap();
        let next = apply_update(
            &channel,
            UpdateImChannelRequest {
                workspace: Some("   ".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(next.workspace, None);
    }

    #[test]
    fn capacity_enforces_the_contract_constant() {
        ensure_channel_capacity(MAX_IM_CHANNELS - 1).unwrap();
        let err = ensure_channel_capacity(MAX_IM_CHANNELS).unwrap_err();
        assert_eq!(err.code, ArkErrorCode::ValidationError);
    }

    #[test]
    fn inbound_text_is_clamped_to_the_wire_limit() {
        let text = "x".repeat(ark_contract::MAX_TURN_TEXT_BYTES + 10);
        assert_eq!(
            truncate_inbound(&text).len(),
            ark_contract::MAX_TURN_TEXT_BYTES
        );
    }

    #[test]
    fn autonomy_defaults_to_unattended() {
        let mut channel = prepare_create(CreateImChannelRequest {
            kind: ImChannelKind::Telegram,
            title: Some("bot".into()),
            credentials: Some(telegram_credentials()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(channel_autonomy(&channel), "unattended");
        channel.autonomy = Some("strict".into());
        assert_eq!(channel_autonomy(&channel), "strict");
    }
}
