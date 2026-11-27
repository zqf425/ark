//! `shell_session` tests.
//!
//! Split from the implementation file to honor the 1000-line limit
//! (AGENTS.md 1000-line limit): the session cwd parser and the background-task
//! registry each carry their own failure catalogue, and both are tested against
//! the same fixture helpers. `manager_tests` covers `ShellSessionManager` state
//! folding; `tool_tests` covers the three tool facades end to end.

use super::*;

#[cfg(test)]
mod manager_tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    #[test]
    fn cwd_starts_at_the_workspace() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cwd-start");
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn cd_advances_only_after_commit() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-advance");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let target = std::fs::canonicalize(ws.join("src")).unwrap();
        let prepared = manager.prepare(&ws, "cd src");
        assert_eq!(prepared.cwd, ws, "a pure cd runs once from the old cwd");
        assert_eq!(prepared.command, "cd src");
        assert_eq!(prepared.commit_target.as_deref(), Some(target.as_path()));
        assert_eq!(
            manager.cwd(&ws),
            ws,
            "prepare must not mutate session state"
        );
        manager.commit(&ws, prepared.commit_target.as_deref());
        assert_eq!(manager.cwd(&ws), target);
        let again = manager.prepare(&ws, "echo hi");
        assert_eq!(again.cwd, target);
        assert_eq!(again.commit_target, None);
    }

    #[test]
    fn absolute_cd_outside_the_workspace_is_ignored() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-escape");
        let prepared = manager.prepare(&ws, "cd /etc");
        assert_eq!(
            prepared.cwd, ws,
            "an escape attempt must not move the session"
        );
        assert_eq!(prepared.commit_target, None);
    }

    #[test]
    fn cd_dash_and_bare_cd_keep_the_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-dash");
        for command in ["cd -", "cd"] {
            let prepared = manager.prepare(&ws, command);
            assert_eq!(prepared.cwd, ws);
            assert_eq!(prepared.commit_target, None);
        }
    }

    #[test]
    fn non_leading_and_compound_cd_keep_the_original_script() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-nonleading");
        for command in [
            "echo hi && cd src",
            "cd src && pwd",
            "cd src; pwd",
            "cd src;",
        ] {
            let prepared = manager.prepare(&ws, command);
            assert_eq!(prepared.cwd, ws);
            assert_eq!(prepared.command, command);
            assert_eq!(prepared.commit_target, None);
        }
    }

    #[test]
    fn quoted_cd_target_is_unquoted_without_concatenation() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-quoted");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let target = std::fs::canonicalize(ws.join("src")).unwrap();
        let prepared = manager.prepare(&ws, "cd \"src\"");
        assert_eq!(prepared.commit_target.as_deref(), Some(target.as_path()));
        std::fs::create_dir_all(ws.join("with space")).unwrap();
        let spaced = manager.prepare(&ws, "cd \"with space\"");
        assert_eq!(
            spaced.commit_target.as_deref(),
            Some(
                std::fs::canonicalize(ws.join("with space"))
                    .unwrap()
                    .as_path()
            )
        );
        for command in ["cd \"src\"other", "cd \"src\" other"] {
            let concatenated = manager.prepare(&ws, command);
            assert_eq!(concatenated.command, command);
            assert_eq!(concatenated.commit_target, None);
        }
    }

    #[test]
    fn variable_cd_target_is_not_recorded() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-var");
        let prepared = manager.prepare(&ws, "cd \"$USERPROFILE/AppData\"");
        assert_eq!(
            prepared.cwd, ws,
            "an unexpanded variable must not move the session"
        );
        assert_eq!(prepared.commit_target, None);
        assert_eq!(manager.cwd(&ws), ws, "the session state must stay clean");
    }

    #[test]
    fn an_unusable_cd_target_falls_back_to_the_current_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-unusable");
        std::fs::write(ws.join("file.txt"), b"x").unwrap();
        for command in ["cd file.txt", "cd nosuchdir"] {
            let prepared = manager.prepare(&ws, command);
            assert_eq!(prepared.cwd, ws);
            assert_eq!(prepared.commit_target, None);
            assert_eq!(manager.cwd(&ws), ws);
        }
    }

    #[test]
    fn quoted_edge_targets_stay_safe() {
        let ws = fixture("cd-edges");
        std::fs::create_dir_all(ws.join("a")).unwrap();
        let manager = ShellSessionManager::new();
        for command in ["cd 'a", "cd \"\"", "cd-x a", "cd \"a\"b"] {
            let prepared = manager.prepare(&ws, command);
            assert_eq!(prepared.cwd, ws, "{command}");
            assert_eq!(prepared.commit_target, None, "{command}");
        }
        let prepared = manager.prepare(&ws, "\n  cd a");
        assert_eq!(
            prepared.commit_target,
            Some(std::fs::canonicalize(ws.join("a")).unwrap())
        );
    }

    #[test]
    fn multiline_leading_cd_runs_only_the_remainder_from_the_target() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-multiline");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let target = std::fs::canonicalize(ws.join("src")).unwrap();
        let prepared = manager.prepare(&ws, "\ncd src\npwd\nprintf done");
        assert_eq!(prepared.cwd, target);
        assert_eq!(prepared.command, "pwd\nprintf done");
        assert_eq!(prepared.commit_target.as_deref(), Some(target.as_path()));
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cd_target_cannot_escape_the_workspace() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-symlink");
        let outside = fixture("cd-symlink-outside");
        std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
        let prepared = manager.prepare(&ws, "cd link\npwd");
        assert_eq!(prepared.cwd, ws);
        assert_eq!(prepared.command, "cd link\npwd");
        assert_eq!(prepared.commit_target, None);
        manager.commit(&ws, prepared.commit_target.as_deref());
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn cd_dot_does_not_accumulate_segments() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-dot");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let target = std::fs::canonicalize(ws.join("src")).unwrap();
        let first = manager.prepare(&ws, "cd src");
        manager.commit(&ws, first.commit_target.as_deref());
        let dot = manager.prepare(&ws, "cd .");
        assert_eq!(dot.cwd, target);
        manager.commit(&ws, dot.commit_target.as_deref());
        assert_eq!(manager.cwd(&ws), target);
    }

    #[test]
    fn a_stale_session_cwd_falls_back_to_the_workspace() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-stale");
        std::fs::create_dir_all(ws.join("gone")).unwrap();
        let first = manager.prepare(&ws, "cd gone");
        manager.commit(&ws, first.commit_target.as_deref());
        std::fs::remove_dir_all(ws.join("gone")).unwrap();
        let prepared = manager.prepare(&ws, "echo hi");
        assert_eq!(prepared.cwd, ws);
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn pushd_advances_like_cd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("pushd");
        std::fs::create_dir_all(ws.join("lib")).unwrap();
        let prepared = manager.prepare(&ws, "pushd lib");
        assert_eq!(prepared.command, "pushd lib");
        assert_eq!(
            prepared.commit_target,
            Some(std::fs::canonicalize(ws.join("lib")).unwrap())
        );
    }

    #[test]
    fn task_ids_are_unique() {
        let manager = ShellSessionManager::new();
        let ws = fixture("task-ids");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id_a = manager
            .register_task(
                ws.clone(),
                shared_child(),
                orphan_tree(),
                Arc::clone(&output),
            )
            .unwrap();
        let id_b = manager
            .register_task(ws, shared_child(), orphan_tree(), output)
            .unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn unknown_task_id_is_none() {
        let manager = ShellSessionManager::new();
        assert!(manager
            .task_output(&ShellTaskId::new("task_nope"))
            .is_none());
        assert!(!manager.kill_task(&ShellTaskId::new("task_nope")));
    }

    #[test]
    fn output_is_incremental_across_polls() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        output.lock().unwrap().push(b"first\n");
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output, "first\n");
        output.lock().unwrap().push(b"second\n");
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "second\n");
        assert_eq!(second.full_text, "first\nsecond\n");
    }

    #[test]
    fn incremental_output_keeps_utf8_split_between_collector_reads() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental-utf8");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        let character = "中".as_bytes();
        output.lock().unwrap().push(&character[..2]);
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output, "");
        assert!(!first.full_text.contains('\u{fffd}'));
        output.lock().unwrap().push(&character[2..]);
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "中");
        assert_eq!(second.full_text, "中");
    }

    #[test]
    fn incremental_output_keeps_gb18030_split_between_collector_reads() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental-gb18030");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        let character = [0x95_u8, 0x32, 0x82, 0x36];
        output.lock().unwrap().push(&character[..3]);
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output, "");
        output.lock().unwrap().push(&character[3..]);
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "𠀀");
        assert_eq!(second.full_text, "𠀀");
    }

    #[test]
    fn overflowed_output_drops_the_oldest_bytes() {
        let mut output = TaskOutput::default();
        let big = vec![b'x'; TASK_OUTPUT_CAP + 10];
        output.push(&big);
        assert!(output.dropped_prefix);
        assert_eq!(output.buf.len(), TASK_OUTPUT_CAP);
    }

    #[test]
    fn incremental_output_survives_ring_buffer_rotation() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental-overflow");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        output.lock().unwrap().push(&vec![b'x'; TASK_OUTPUT_CAP]);
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output.len(), TASK_OUTPUT_CAP);
        output.lock().unwrap().push(b"new-tail");
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "new-tail");
    }

    #[test]
    fn registering_a_seventeenth_running_task_is_rejected() {
        let manager = ShellSessionManager::new();
        let ws = fixture("task-capacity");
        let mut outputs = Vec::new();
        for _ in 0..MAX_TASKS {
            let output = Arc::new(Mutex::new(TaskOutput::default()));
            manager
                .register_task(
                    ws.clone(),
                    shared_child(),
                    orphan_tree(),
                    Arc::clone(&output),
                )
                .expect("task within capacity");
            outputs.push(output);
        }
        let overflow = manager.register_task(
            ws,
            shared_child(),
            orphan_tree(),
            Arc::new(Mutex::new(TaskOutput::default())),
        );
        assert!(overflow.is_err());
    }

    #[test]
    fn kill_is_idempotent_after_finish() {
        let manager = ShellSessionManager::new();
        let ws = fixture("kill-idempotent");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), output)
            .unwrap();
        // The fake child is a real process that exits immediately; either kill
        // result is acceptable, but a second kill must not panic or hang.
        let _ = manager.kill_task(&id);
        let _ = manager.kill_task(&id);
        assert!(manager.has_task(&id));
    }

    /// A real child (instant exit) in the shared registry shape.
    fn shared_child() -> Arc<Mutex<Option<std::process::Child>>> {
        let child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                vec!["/C", "exit 0"]
            } else {
                vec![]
            })
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn fake child");
        Arc::new(Mutex::new(Some(child)))
    }

    /// An un-adopted tree handle: the fake child never ran isolated, so group
    /// signals report false and the direct-kill fallback covers teardown.
    fn orphan_tree() -> ark_common::ProcessTree {
        ark_common::ProcessTree::adopt(None)
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use ark_ports::{SandboxDecision, ToolContext};
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn ctx(root: &Path) -> ToolContext {
        ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: root.to_path_buf(),
            sandbox: SandboxDecision::Unrestricted,
            env: BTreeMap::new(),
        }
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-shelltool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    #[tokio::test]
    async fn foreground_command_runs_and_advances_cwd() {
        let dir = fixture("fg");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let target = std::fs::canonicalize(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        // A pure cd must execute once from the old cwd, not once from the
        // already-advanced target (which used to fail when `src/src` was absent).
        let changed = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "cd src"})),
            )
            .await
            .unwrap();
        assert!(changed.ok, "{}", changed.content);
        assert_eq!(session.cwd(&dir), target);
        let plain = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "echo hi"})),
            )
            .await
            .unwrap();
        assert!(plain.ok, "{}", plain.content);
        assert_eq!(session.cwd(&dir), target);
    }

    #[tokio::test]
    async fn multiline_cd_runs_the_remainder_once_and_commits_on_success() {
        let dir = fixture("fg-multiline");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let target = std::fs::canonicalize(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let out = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "cd src\npwd"})),
            )
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        // `pwd` prints a POSIX path (`/c/...`, or `/tmp/...` where MSYS mounts
        // the temp dir), which can never contain the verbatim Windows form
        // `\\?\C:\...` that `canonicalize` returns — a verbatim-substring
        // assertion is always false here. Assert the unique fixture leaf
        // instead; the exact canonical identity is already pinned below via
        // `session.cwd`.
        assert!(
            out.content.contains(&format!(
                "ark-shelltool-fg-multiline-{}",
                std::process::id()
            )),
            "pwd output must name the fixture leaf: {}",
            out.content
        );
        assert!(!out.content.contains("[stderr]"), "{}", out.content);
        assert_eq!(session.cwd(&dir), target);
    }

    #[tokio::test]
    async fn failed_foreground_commands_do_not_advance_cwd() {
        let dir = fixture("fg-failed");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        for command in ["cd src\nfalse", "cd missing"] {
            let out = tool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({"command": command})),
                )
                .await
                .unwrap();
            assert!(!out.ok, "{command}");
            assert_eq!(session.cwd(&dir), dir, "{command}");
        }
    }

    #[tokio::test]
    async fn pure_pushd_keeps_its_output_and_compound_cd_is_not_tracked() {
        let dir = fixture("fg-pushd");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let target = std::fs::canonicalize(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let pushed = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "pushd src"})),
            )
            .await
            .unwrap();
        assert!(pushed.ok, "{}", pushed.content);
        assert!(pushed.content.contains("src"), "{}", pushed.content);
        assert_eq!(session.cwd(&dir), target);

        let fresh = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&fresh));
        for command in ["cd src && pwd", "cd src; pwd"] {
            let out = tool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({"command": command})),
                )
                .await
                .unwrap();
            assert!(out.ok, "{}", out.content);
            assert_eq!(fresh.cwd(&dir), dir, "{command}");
        }
    }

    #[tokio::test]
    async fn background_leading_cd_commits_after_registration_without_double_cd() {
        let dir = fixture("bg-cd");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let target = std::fs::canonicalize(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let output_tool = BashOutputTool::new(Arc::clone(&session));
        let started = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({
                    "command": "cd src\npwd",
                    "run_in_background": true
                })),
            )
            .await
            .unwrap();
        assert!(started.ok, "{}", started.content);
        assert_eq!(session.cwd(&dir), target);
        let task_id = started
            .content
            .lines()
            .find_map(|line| line.strip_prefix("background task started: "))
            .expect("task id line")
            .trim()
            .to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        loop {
            let snapshot = output_tool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({"task_id": task_id})),
                )
                .await
                .unwrap();
            seen.push_str(&snapshot.content);
            if seen.contains("task finished") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never finished: {seen}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Same POSIX-vs-verbatim rule as the foreground multiline test: `pwd`
        // prints a POSIX path, so assert the unique fixture leaf instead of the
        // verbatim Windows form; the canonical identity is pinned above via
        // `session.cwd`.
        assert!(
            seen.contains(&format!("ark-shelltool-bg-cd-{}", std::process::id())),
            "pwd output must name the fixture leaf: {seen}"
        );
        assert!(!seen.contains("can't cd"), "{seen}");
    }

    #[tokio::test]
    async fn background_task_returns_an_id_and_collects_output() {
        let dir = fixture("bg");
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let output_tool = BashOutputTool::new(Arc::clone(&session));
        let started = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({
                    "command": "echo bg-hello",
                    "run_in_background": true
                })),
            )
            .await
            .unwrap();
        assert!(started.ok, "{}", started.content);
        let task_id = started
            .content
            .lines()
            .find_map(|line| line.strip_prefix("background task started: "))
            .expect("task id line")
            .trim()
            .to_string();

        // Poll until the collector observes the echo; bounded wait.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        loop {
            let snapshot = output_tool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({ "task_id": task_id })),
                )
                .await
                .unwrap();
            seen.push_str(&snapshot.content);
            if snapshot.content.contains("bg-hello") || snapshot.content.contains("finished") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never saw output: {seen}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(seen.contains("bg-hello"), "{seen}");
        assert!(
            seen.contains("task finished") || seen.contains("task running"),
            "{seen}"
        );
    }

    #[tokio::test]
    async fn unknown_task_output_is_a_structured_error() {
        let dir = fixture("unknown");
        let tool = BashOutputTool::new(Arc::new(ShellSessionManager::new()));
        let out = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": "task_999" })),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("unknown task id"));
        // The rejection must teach the two namespaces: a model that passed a
        // delegate_task child id has to learn that the answer arrives as a
        // completion notice instead of re-running the work itself.
        assert!(
            out.content.contains("bash_session(run_in_background=true)"),
            "{}",
            out.content
        );
        assert!(out.content.contains("delegate_task"), "{}", out.content);
        assert!(out.content.contains("completion notice"), "{}", out.content);
        assert!(out.content.contains("bash_output"), "{}", out.content);
        // The notice only wakes a turn started after the child finishes; the
        // rejection must say to end the turn, not to keep waiting in place.
        assert!(out.content.contains("end your turn"), "{}", out.content);
    }

    #[tokio::test]
    async fn kill_unknown_task_is_a_structured_error() {
        let dir = fixture("kill-unknown");
        let tool = BashKillTool::new(Arc::new(ShellSessionManager::new()));
        let out = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": "task_1" })),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("unknown task id"));
        assert!(out.content.contains("bash_kill"), "{}", out.content);
        assert!(
            out.content.contains("bash_session(run_in_background=true)"),
            "{}",
            out.content
        );
        assert!(out.content.contains("delegate_task"), "{}", out.content);
    }

    #[tokio::test]
    async fn a_long_background_task_is_killable() {
        let dir = fixture("kill-running");
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let kill_tool = BashKillTool::new(Arc::clone(&session));
        let long = "sleep 30";
        let started = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "command": long, "run_in_background": true })),
            )
            .await
            .unwrap();
        let task_id = started
            .content
            .lines()
            .find_map(|line| line.strip_prefix("background task started: "))
            .expect("task id line")
            .trim()
            .to_string();
        let killed = kill_tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": task_id })),
            )
            .await
            .unwrap();
        assert!(killed.ok, "{}", killed.content);
        assert!(killed.content.contains("terminated"));
    }
}
