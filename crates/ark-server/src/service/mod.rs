//! Transport services: the listener and its graceful shutdown path.

pub mod attachment_auth;
pub mod compaction;
pub mod continuation;
pub mod im;
pub mod im_inbound;
pub mod mcp_info;
pub mod mcp_tools;
pub mod scheduler;
pub mod serve;
pub mod thread_titles;
pub mod threads;
pub mod turns;
pub mod workbench;
pub mod workspace_projects;

pub use attachment_auth::{
    associated_attachment_ids, authorize_turn_attachments, thread_attachment_ids,
};
pub use compaction::{ensure_checkpoint, CompactionOutcome};
pub use continuation::{schedule_auto_continuation, sync_auto_continue_policy};
pub use im::ImService;
pub use scheduler::{SchedulerService, TICK_INTERVAL as SCHEDULE_TICK_INTERVAL};
pub use serve::{bind, AppHandle};
pub use thread_titles::name_thread_on_first_turn;
pub use threads::{create_thread, delete_settled_by_workspace, summary_of};
pub use turns::{spawn_turn, start_turn, start_turn_as};
