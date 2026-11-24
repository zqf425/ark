//! Context utilities: the single token estimator and history hygiene.

pub mod checkpoint;
pub mod completion;
pub mod hygiene;
pub mod replay;
pub mod token;

pub use checkpoint::ContextCheckpoint;
pub use completion::{
    auto_delivery_attempts, consecutive_auto_model_request_failures,
    consecutive_model_request_failures, CompletionInbox, MAX_AUTO_DELIVERY_ATTEMPTS,
    RECOVERY_MODEL_FAILURE_STREAK_LIMIT,
};
pub use hygiene::{HistoryHygiene, HygieneReport};
pub use replay::model_history;
pub use token::TokenEstimator;

#[cfg(test)]
#[path = "checkpoint_evidence_tests.rs"]
mod checkpoint_evidence_tests;

#[cfg(test)]
mod checkpoint_tests {
    use super::checkpoint::MAX_PLAN_LINES;
    use super::ContextCheckpoint;
    use ark_ports::ModelMessage;

    /// Tool 角色测试桩：与 reactor 回放 `ToolCall` 结果时的消息形态一致。
    fn tool_result(output: &str) -> ModelMessage {
        ModelMessage {
            role: ark_ports::ModelRole::Tool,
            content: output.to_string(),
            images: Vec::new(),
            tool_call_id: Some("call_test".to_string()),
            name: Some("bash".to_string()),
            tool_calls: Vec::new(),
        }
    }

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

    #[test]
    fn plan_restate_survives_when_plan_rows_come_after_a_full_entries_quota() {
        // N1: plan rows sit at the seed tail, after earlier structured facts.
        // With 40+ older entries, a take-before-filter scan would drop every
        // plan row exactly in the longest turns where the restate matters.
        let mut texts: Vec<String> = (0..50).map(|i| format!("Decision: d{i}")).collect();
        texts.push("Goal: repair the parser".to_string());
        texts.push("- [ ] run tests".to_string());
        let checkpoint = ContextCheckpoint::from_texts(texts.iter().map(String::as_str));

        let restate = checkpoint.plan_restate().expect("plan rows survive quota");
        assert!(restate.contains("Goal: repair the parser"));
        assert!(restate.contains("- [ ] run tests"));
        assert!(!restate.contains("Decision:"));
    }

    #[test]
    fn plan_restate_folds_status_variants_last_wins_by_title() {
        // N2: the same todo appears in `entries` once per rendered status
        // (`- [ ] run tests` before completion, `- [x] run tests` after).
        // Restating both would show the model a self-contradicting plan; the
        // latest occurrence wins per title, `Goal:` included.
        let checkpoint = ContextCheckpoint::from_texts([
            "Goal: old objective",
            "Goal: repair the parser",
            "- [ ] run tests",
            "- [x] run tests",
            "- [~] fix wire format",
        ]);

        let restate = checkpoint.plan_restate().expect("plan rows exist");
        assert!(restate.contains("Goal: repair the parser"));
        assert!(!restate.contains("old objective"));
        assert!(restate.contains("- [x] run tests"));
        assert!(!restate.contains("- [ ] run tests"));
        assert!(restate.contains("- [~] fix wire format"));
    }

    #[test]
    fn plan_restate_does_not_eat_leading_x_of_a_title() {
        // M3: char-set stripping ate a title's leading `x`, colliding
        // `- [ ] xtask migration` with `- [ ] task migration` and silently
        // folding two different todos into one. Exact marker prefixes only.
        let checkpoint =
            ContextCheckpoint::from_texts(["- [ ] xtask migration", "- [ ] task migration"]);

        let restate = checkpoint.plan_restate().expect("plan rows exist");
        assert!(restate.contains("- [ ] xtask migration"));
        assert!(restate.contains("- [ ] task migration"));
    }

    #[test]
    fn plan_restate_keeps_the_newest_goal_first_and_marks_quota_truncation() {
        // M5: the objective must never be dropped by the todo quota, and a
        // quota cut is announced instead of silent (the checkpoint budget
        // render sets the same precedent).
        let mut texts: Vec<String> = (0..(MAX_PLAN_LINES))
            .map(|i| format!("- [ ] todo {i}"))
            .collect();
        texts.push("Goal: the real objective".to_string());
        let checkpoint = ContextCheckpoint::from_texts(texts.iter().map(String::as_str));

        let restate = checkpoint.plan_restate().expect("plan rows exist");
        assert!(restate.starts_with("[thread plan]\nGoal: the real objective"));
        assert!(restate.contains("truncated"));
        // The goal occupies one slot, so todos 0..=38 survive (39 of 40);
        // todo 39 is the first behind the cut.
        assert!(restate.contains(&format!("- [ ] todo {}", MAX_PLAN_LINES - 2)));
        assert!(!restate.contains("- [ ] todo 39"));
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

        let rendered = checkpoint.render_with_token_limit(21);
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
        assert!(rendered.contains("(repeated x2)"));
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

    /// S1：`failed:` 后跟真实诊断描述必须存活——该形态是工具失败的主要
    /// 前缀，整行拒绝会让失败证据段对它完全失明。
    #[test]
    fn genuine_failed_diagnostics_survive_code_shaped_filter() {
        let checkpoint = ContextCheckpoint::from_texts([
            "failed: connect 127.0.0.1:18792 refused",
            "failed: exit code 101",
            "failed to compile module",
            "failed: Boolean(failed),",
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("connect 127.0.0.1:18792 refused"));
        assert!(rendered.contains("exit code 101"));
        assert!(rendered.contains("failed to compile module"));
        assert!(!rendered.contains("Boolean(failed)"));
    }

    /// S2：同一条 Tool 失败出现两次必须归并为一条，数字变体同样归并——
    /// Tool 结果是错误的主要来源，双通道写入会让归并失效。
    #[test]
    fn tool_failure_duplicates_and_variants_collapse() {
        let checkpoint = ContextCheckpoint::from_messages(&[
            tool_result("Error: exit status 101"),
            tool_result("Error: exit status 101"),
        ]);

        let rendered = checkpoint.render();
        assert_eq!(rendered.matches("exit status 101").count(), 1);
        assert!(rendered.contains("(repeated x2)"));

        let variants = ContextCheckpoint::from_messages(&[
            tool_result("Error: HTTP 401 unauthorized"),
            tool_result("Error: HTTP 404 not found"),
        ]);
        assert_eq!(variants.render().matches("HTTP 4").count(), 2);
    }

    /// S3：手动 /compact 每次重读全部 items,同一条失败证据被重复吸收；
    /// absorb 必须幂等,不得把既有条目再 bump 出假计数。
    #[test]
    fn repeated_checkpoint_absorption_is_idempotent() {
        let messages = vec![tool_result("Error: exit status 101")];
        let first = ContextCheckpoint::from_messages(&messages).render();
        let second = ContextCheckpoint::from_messages(&[
            ModelMessage::system(&first),
            ModelMessage::system(&first),
        ]);

        let rendered = second.render();
        assert!(!rendered.contains("(repeated x2)"));
        assert_eq!(rendered.matches("exit status 101").count(), 1);
    }

    /// M1：`render_with_token_limit` 的截断变体头同样会被吸收；截断渲染
    /// 保留段标题，吸收后内容不丢。
    #[test]
    fn truncated_checkpoint_absorption_keeps_user_requests() {
        let mut messages: Vec<ModelMessage> =
            vec![ModelMessage::user("原始目标：完善原子库并优化召回逻辑")];
        messages.extend((0..20).map(|i| ModelMessage::user(format!("req-{i:02} QUARTZ-99"))));
        let truncated = ContextCheckpoint::from_messages(&messages).render_with_token_limit(60);
        assert!(truncated.contains("truncated"));

        let next = ContextCheckpoint::from_messages(&[
            ModelMessage::system(&truncated),
            ModelMessage::user("继续验证只读用例"),
        ]);
        let rendered = next.render();
        assert!(rendered.contains("原始目标：完善原子库并优化召回逻辑"));
        assert!(rendered.contains("继续验证只读用例"));
    }

    /// M2：fold 账目行再吸收时必须丢弃，不占用户请求配额。
    #[test]
    fn fold_marker_lines_are_dropped_on_absorption() {
        let messages: Vec<ModelMessage> = (0..60)
            .map(|index| ModelMessage::user(format!("request-{index:03} with QUARTZ-99")))
            .collect();
        let first = ContextCheckpoint::from_messages(&messages).render();
        let next = ContextCheckpoint::from_messages(&[ModelMessage::system(&first)]);

        let rendered = next.render();
        assert!(!rendered.contains("folded - see the event log"));
        assert!(rendered.contains("request-000"));
    }

    /// M4：行尾恰为 `[xN]` 形态的字面数据不得被当作计数器改写。
    #[test]
    fn literal_xn_suffix_is_not_treated_as_a_counter() {
        let checkpoint = ContextCheckpoint::from_texts([
            "ERROR: crc mismatch [x86]",
            "ERROR: crc mismatch [x87]",
        ]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("[x86]"));
        assert!(rendered.contains("[x87]"));
        assert!(!rendered.contains("(repeated x2)"));
    }

    /// M5：rustc 诊断携带 `=> ` 闭包签名时必须存活。
    #[test]
    fn rustc_diagnostics_with_arrow_survive() {
        let checkpoint = ContextCheckpoint::from_texts(["error[E0277]: expected `Fn(u8) => bool`"]);
        assert!(checkpoint.render().contains("error[E0277]"));
    }

    /// M6：JS 序列化键首字母大写的代码行同样要被拒绝。
    #[test]
    fn capitalized_key_value_code_lines_are_rejected() {
        let checkpoint = ContextCheckpoint::from_texts(["Failure: null,", "Failed: null,"]);
        let rendered = checkpoint.render();
        assert!(!rendered.contains("Failure: null"));
        assert!(!rendered.contains("Failed: null"));
    }

    /// B1：absorb 失败段时过滤非诊断行，与 from_texts 行为对称。
    #[test]
    fn absorption_applies_failure_filter() {
        let old =
            ContextCheckpoint::from_texts(["Error: ENOENT", "retry the deploy later"]).render();
        let next = ContextCheckpoint::from_messages(&[ModelMessage::system(&old)]);
        let rendered = next.render();
        assert!(rendered.contains("Error: ENOENT"));
        assert!(!rendered.contains("retry the deploy later"));
    }

    /// N1：单引号 JS 值行必须被拒绝（recheck 报告确认 `failed: 'x',` 泄漏）。
    #[test]
    fn single_quoted_js_values_are_rejected() {
        let checkpoint = ContextCheckpoint::from_texts(["failed: 'x',", "Failure: 'y',"]);
        let rendered = checkpoint.render();
        assert!(!rendered.contains("'x'"));
        assert!(!rendered.contains("'y'"));
    }

    /// N2：组合词 `actionCount` 必须触发计数语境归并（recheck 报告实证
    /// 整 token 精确比较时两个数字都变的场景不归并）。
    #[test]
    fn compound_counting_words_trigger_collapse() {
        let checkpoint = ContextCheckpoint::from_texts([
            "ERROR: actionCount=27, actual 28",
            "ERROR: actionCount=29, actual 30",
        ]);
        let rendered = checkpoint.render();
        assert!(rendered.contains("(repeated x2)"));
        assert_eq!(rendered.matches("ERROR:").count(), 1);
    }

    /// N3：截断渲染中某段首行放不下时，同段后续行必须仍带本段标题或被裁，
    /// 不得悬挂在上一段标题下（否则吸收后错段）。
    #[test]
    fn truncated_render_never_dangles_bullets_under_the_wrong_title() {
        let messages = vec![
            ModelMessage::user("short-goal QUARTZ-99"),
            // 失败段的第一条极长（在低预算下放不下），后续条较短。
            tool_result(
                "Error: a very long failure line that certainly will not fit into a small budget at all",
            ),
            tool_result("Error: short"),
        ];
        let checkpoint = ContextCheckpoint::from_messages(&messages);

        for budget in 12..=40 {
            let rendered = checkpoint.render_with_token_limit(budget);
            let mut current = "";
            for line in rendered.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    current = trimmed;
                } else if trimmed.starts_with("- Error") {
                    assert_eq!(
                        current, "[failed attempts - do not repeat]",
                        "budget {budget}: dangling failure bullet under {current}"
                    );
                }
            }
        }
    }

    /// B5 残余盲区：三代连续压缩（两次再吸收）后首条用户请求与失败证据
    /// 必须完整存活，计数链单调不分裂。
    #[test]
    fn three_generation_compaction_chain_keeps_original_goal() {
        let original = ContextCheckpoint::from_messages(&[
            ModelMessage::user("原始目标：完善原子库并优化召回逻辑 QUARTZ-99"),
            tool_result("Error: exit status 101"),
            tool_result("Error: exit status 101"),
        ]);
        let gen1 = ContextCheckpoint::from_messages(&[
            ModelMessage::system(original.render()),
            ModelMessage::user("继续验证后面的只读安全用例"),
        ]);
        let gen2 = ContextCheckpoint::from_messages(&[
            ModelMessage::system(gen1.render()),
            ModelMessage::user("跳过异常监控模块的用例"),
        ]);
        let gen3 = ContextCheckpoint::from_messages(&[
            ModelMessage::system(gen2.render()),
            ModelMessage::user("先测试 system-setting 模块"),
        ]);

        let rendered = gen3.render();
        assert!(rendered.contains("原始目标：完善原子库并优化召回逻辑"));
        assert!(rendered.contains("继续验证后面的只读安全用例"));
        assert!(rendered.contains("先测试 system-setting 模块"));
        assert_eq!(rendered.matches("exit status 101").count(), 1);
        // 计数链:原始代两次失败 → x2;此后三代吸收均幂等不 bump(S3 语义),
        // 没有新的真实失败,计数保持 x2 不虚增。
        assert!(rendered.contains("(repeated x2)"));
    }
}
