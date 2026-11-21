//! Context utilities: the single token estimator and history hygiene.

pub mod checkpoint;
pub mod completion;
pub mod hygiene;
pub mod token;

pub use checkpoint::ContextCheckpoint;
pub use completion::{
    auto_delivery_attempts, consecutive_auto_model_request_failures,
    consecutive_model_request_failures, CompletionInbox, MAX_AUTO_DELIVERY_ATTEMPTS,
    RECOVERY_MODEL_FAILURE_STREAK_LIMIT,
};
pub use hygiene::{HistoryHygiene, HygieneReport};
pub use token::TokenEstimator;

#[cfg(test)]
mod checkpoint_tests {
    use super::ContextCheckpoint;
    use ark_ports::ModelMessage;

    #[test]
    fn checkpoint_keeps_goal_constraints_and_recent_decision() {
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("Goal: repair the parser"),
            ModelMessage::assistant("Decision: preserve the public API"),
            ModelMessage::user("Constraint: do not change the wire format"),
        ]);

        assert!(checkpoint.render().contains("Goal: repair the parser"));
        assert!(checkpoint
            .render()
            .contains("Constraint: do not change the wire format"));
        assert!(checkpoint
            .render()
            .contains("Decision: preserve the public API"));
    }

    #[test]
    fn checkpoint_keeps_short_identifiers_requested_by_the_user() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::user(
            "Remember these passwords: ZEBRA-77 and QUARTZ-99.",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("ZEBRA-77"));
        assert!(rendered.contains("QUARTZ-99"));
    }

    #[test]
    fn checkpoint_keeps_short_quoted_strings_without_treating_plain_words_as_identifiers() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::user(
            "Keep 'db-pass' and \"release-candidate\" for the deploy.",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("db-pass"));
        assert!(rendered.contains("release-candidate"));
        assert!(!rendered.contains("- deploy"));
    }

    #[test]
    fn checkpoint_rejects_empty_or_overlong_quoted_strings() {
        let too_long = "a".repeat(81);
        let checkpoint =
            ContextCheckpoint::from_texts([format!("Keep '' and \"{too_long}\"").as_str()]);

        let rendered = checkpoint.render();
        assert!(!rendered.contains(&too_long));
    }

    #[test]
    fn checkpoint_keeps_plan_todo_lines_from_the_seed() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::system(
            "[thread plan - current objective and progress]\n\
             Goal: repair the parser\n\
             - [x] read files\n\
             - [~] fix wire format\n\
             - [ ] run tests",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("Goal: repair the parser"));
        assert!(rendered.contains("- [x] read files"));
        assert!(rendered.contains("- [~] fix wire format"));
        assert!(rendered.contains("- [ ] run tests"));
    }

    #[test]
    fn checkpoint_merges_only_facts_from_a_generated_checkpoint() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::system(
            "[context checkpoint]\n[remembered identifiers]\n- ZEBRA-77\nGoal: preserve legacy fact\n[3 earlier messages compacted]",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.starts_with("[context checkpoint]"));
        assert!(rendered.contains("Goal: preserve legacy fact"));
        assert!(rendered.contains("ZEBRA-77"));
        // Bookkeeping lines from a prior note are never re-emitted verbatim.
        assert!(!rendered.contains("[3 earlier messages compacted]"));
    }

    #[test]
    fn checkpoint_keeps_user_requests_and_failure_evidence() {
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("please migrate the parser module"),
            ModelMessage::assistant("I will read the files first"),
            ModelMessage::system("Error: `cargo test` failed with exit code 101"),
            ModelMessage::user("Next: rerun after fixing the wire format"),
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("please migrate the parser module"));
        assert!(rendered.contains("[user requests]"));
        assert!(rendered.contains("[failed attempts - do not repeat]"));
        assert!(rendered.contains("cargo test` failed"));
        assert!(rendered.contains("Next: rerun after fixing the wire format"));
    }

    #[test]
    fn checkpoint_summarises_the_kept_tail_too() {
        // The next turn replays only the checkpoint plus newer rows, so the
        // tail that used to be kept verbatim must now be represented here.
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("old request ZEBRA-77"),
            ModelMessage::assistant("old answer"),
            ModelMessage::user("recent request QUARTZ-99"),
            ModelMessage::assistant("recent answer"),
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("old request ZEBRA-77"));
        assert!(rendered.contains("recent request QUARTZ-99"));
    }

    #[test]
    fn checkpoint_budget_truncates_on_utf8_boundaries() {
        let checkpoint = ContextCheckpoint::from_texts([
            "Goal: 保留足够长的上下文事实，以验证预算裁剪不会截断 UTF-8 字符。",
            "Remember QUARTZ-99 and ZEBRA-77.",
        ]);

        let rendered = checkpoint.render_with_token_limit(12);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
        assert!(rendered.contains("truncated"));
        assert!(crate::context::TokenEstimator::new().estimate_text(&rendered) <= 12);
    }

    /// 回归：第二次压缩必须继承旧 checkpoint 的 `[user requests]` 段。
    /// 实测会话 cb274b9d 中，第二次压缩后原始目标（最早的用户请求，纯文本
    /// 无 `Goal:` 前缀）被静默丢弃，模型从此失去任务锚点。旧摘要行以 `- `
    /// 前缀渲染，`from_texts` 的结构化前缀识别不到，必须由 checkpoint 消息
    /// 的段状态机迁移。
    #[test]
    fn second_compaction_preserves_earliest_user_requests() {
        let first_checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("用百融万相平台的用例完善对应的原子库，同时优化召回逻辑"),
            ModelMessage::assistant("开始逐模块验证"),
        ])
        .render();
        // 第一次压缩后的新历史：旧摘要（system 消息）+ 压缩后新增的消息。
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::system(&first_checkpoint),
            ModelMessage::user("继续验证后面的只读安全用例"),
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("用百融万相平台的用例完善对应的原子库"));
        assert!(rendered.contains("继续验证后面的只读安全用例"));
        // 段标题只出现一次：旧摘要的段被吸收，不重复渲染第二份。
        assert_eq!(rendered.matches("[user requests]").count(), 1);
    }

    /// 回归：旧摘要的 goals 段（含 plan checkbox 行）同样必须迁移。
    #[test]
    fn second_compaction_inherits_goal_and_todo_lines_from_prior_checkpoint() {
        let first_checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::system(
            "[thread plan - current objective and progress]\n\
             Goal: repair the parser\n\
             - [~] fix wire format",
        )])
        .render();
        let checkpoint =
            ContextCheckpoint::from_messages(&[ModelMessage::system(&first_checkpoint)]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("Goal: repair the parser"));
        assert!(rendered.contains("- [~] fix wire format"));
    }

    /// 回归：token 预算截断时用户请求必须先于标识符保留。
    /// 实测摘要里 `Base64`、`P30` 这类碎片挤掉了原始目标。
    #[test]
    fn budget_truncation_keeps_user_requests_before_identifiers() {
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("please migrate the parser module"),
            ModelMessage::assistant("noted P30 QUARTZ-99"),
        ]);

        let rendered = checkpoint.render_with_token_limit(17);
        assert!(rendered.contains("please migrate the parser module"));
        assert!(!rendered.contains("P30"));
    }

    /// 回归：源代码行不得污染失败证据段。实测摘要的
    /// `[failed attempts]` 40 行里混入了 `failed: Boolean(failed),` 等 JS 片段。
    #[test]
    fn failure_collection_excludes_source_code_lines() {
        let checkpoint = ContextCheckpoint::from_texts([
            "failed: Boolean(failed),",
            "error instanceof Error ? safeDisplayText(error.message, 200) : '';",
            "errorMessage: null,",
            "Error: ENOENT: no such file or directory",
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("Error: ENOENT"));
        assert!(!rendered.contains("Boolean(failed)"));
        assert!(!rendered.contains("instanceof"));
        assert!(!rendered.contains("errorMessage:"));
    }

    /// 数字差异是同类错误的典型形态：27/28 与 27/29 折叠为一条并带计数，
    /// 不同的模块保持独立条目。
    #[test]
    fn failure_entries_collapse_numeric_variants() {
        let checkpoint = ContextCheckpoint::from_texts([
            "ERROR A3: 模块 quality-cockpit 的 actionCount=27，实际 28",
            "ERROR A3: 模块 quality-cockpit 的 actionCount=27，实际 29",
            "ERROR A3: 模块 shell 的 actionCount=16，实际 17",
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("[x2]"));
        assert!(rendered.contains("模块 shell"));
        assert_eq!(rendered.matches("ERROR A3").count(), 2);
    }

    /// 回归：用户请求超过段配额时保两端，最旧的目标不得被静默丢弃。
    #[test]
    fn user_requests_overflow_keeps_first_and_last() {
        let messages: Vec<ModelMessage> = (0..60)
            .map(|index| ModelMessage::user(format!("request-{index:03} with QUARTZ-99")))
            .collect();
        let checkpoint = ContextCheckpoint::from_messages(&messages);

        let rendered = checkpoint.render();
        assert!(rendered.contains("request-000"));
        assert!(rendered.contains("request-059"));
        assert!(rendered.contains("folded"));
    }
}
