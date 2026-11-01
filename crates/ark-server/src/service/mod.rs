//! Transport services: the listener and its graceful shutdown path.

pub mod attachment_auth;
pub mod compaction;
pub mod continuation;
pub mod mcp_info;
pub mod mcp_tools;
pub mod scheduler;
pub mod serve;
pub mod threads;
pub mod turns;
pub mod workbench;

pub use attachment_auth::{
    associated_attachment_ids, authorize_turn_attachments, thread_attachment_ids,
};
pub use compaction::{ensure_checkpoint, CompactionOutcome};
pub use continuation::{schedule_auto_continuation, sync_auto_continue_policy};
pub use scheduler::{SchedulerService, TICK_INTERVAL as SCHEDULE_TICK_INTERVAL};
pub use serve::{bind, AppHandle};
pub use threads::{create_thread, summary_of};
pub use turns::{spawn_turn, start_turn};
