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
    fn cd_advances_the_session_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-advance");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let next = manager.resolve_cwd(&ws, "cd src");
        assert_eq!(next, ws.join("src"));
        assert_eq!(manager.cwd(&ws), ws.join("src"));
        // The next call starts from the advanced cwd.
        let again = manager.resolve_cwd(&ws, "echo hi");
        assert_eq!(again, ws.join("src"));
    }

    #[test]
    fn absolute_cd_outside_the_workspace_is_ignored() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-escape");
        let next = manager.resolve_cwd(&ws, "cd /etc");
        assert_eq!(next, ws, "an escape attempt must not move the session");
    }

    #[test]
    fn cd_dash_and_bare_cd_keep_the_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-dash");
        let next = manager.resolve_cwd(&ws, "cd -");
        assert_eq!(next, ws);
        let next = manager.resolve_cwd(&ws, "cd");
        assert_eq!(next, ws);
    }

    #[test]
    fn non_leading_cd_keeps_the_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-nonleading");
        let next = manager.resolve_cwd(&ws, "echo hi && cd src");
        assert_eq!(next, ws);
    }

    #[test]
    fn quoted_cd_target_is_unquoted() {
        // The observed os error 123 cascade: `cd "path"` recorded the quote
        // characters into the session cwd and every later call died on a
        // path no shell could produce.
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-quoted");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let next = manager.resolve_cwd(&ws, "cd \"src\"");
        assert_eq!(next, ws.join("src"));
        assert_eq!(manager.cwd(&ws), ws.join("src"));
    }

    #[test]
    fn variable_cd_target_is_not_recorded() {
        // The shell expands `$USERPROFILE` when the command runs; the parser
        // cannot, so recording the raw `$...` spelling would poison the
        // session cwd exactly like the quote case.
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-var");
        let next = manager.resolve_cwd(&ws, "cd \"$USERPROFILE/AppData\"");
        assert_eq!(next, ws, "an unexpanded variable must not move the session");
        assert_eq!(manager.cwd(&ws), ws, "the session state must stay clean");
        // A later plain command still runs from the untouched cwd.
        assert_eq!(manager.resolve_cwd(&ws, "echo hi"), ws);
    }

    #[test]
    fn an_unusable_cd_target_falls_back_to_the_current_cwd() {
        // The third observed defect shape: a target that is a plain file or
        // missing entirely used to be committed as the session cwd, and every
        // later call then died before its command ran.
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-unusable");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("file.txt"), b"x").unwrap();
        let next = manager.resolve_cwd(&ws, "cd file.txt");
        assert_eq!(next, ws, "a file is not a directory to cd into");
        assert_eq!(manager.cwd(&ws), ws);
        let next = manager.resolve_cwd(&ws, "cd nosuchdir");
        assert_eq!(next, ws, "a missing directory must not be committed");
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn quoted_edge_targets_stay_safe() {
        // Deliberately not a shell tokenizer; every divergence must end in
        // "session cwd stays put", never in a committed wrong path.
        let ws = fixture("cd-edges");
        std::fs::create_dir_all(ws.join("a")).unwrap();
        // Unterminated quote: no target, no advance.
        let manager = ShellSessionManager::new();
        assert_eq!(manager.resolve_cwd(&ws, "cd 'a"), ws);
        // Empty quoted target: no advance.
        let manager = ShellSessionManager::new();
        assert_eq!(manager.resolve_cwd(&ws, "cd \"\""), ws);
        // Verb lookalike `cd-x`: not a cd.
        let manager = ShellSessionManager::new();
        assert_eq!(manager.resolve_cwd(&ws, "cd-x a"), ws);
        // A leading blank line before the cd still counts as leading.
        let manager = ShellSessionManager::new();
        let next = manager.resolve_cwd(&ws, "\n  cd a");
        assert_eq!(next, ws.join("a"));
        // Quote concatenation `cd "a"b`: the parser reads `a`, so the session
        // records `a` instead of the raw `a"b`; the shell's own cwd (`ab`)
        // diverges, which is the accepted under-approximation.
        let manager = ShellSessionManager::new();
        assert_eq!(manager.resolve_cwd(&ws, "cd \"a\"b"), ws.join("a"));
        assert_eq!(manager.cwd(&ws), ws.join("a"));
    }

    #[test]
    fn cd_dot_does_not_accumulate_segments() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-dot");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        assert_eq!(manager.resolve_cwd(&ws, "cd src"), ws.join("src"));
        // `cd .` compares equal after normalization: no `src/.` is committed,
        // so repeated calls cannot grow `src/././.`.
        assert_eq!(manager.resolve_cwd(&ws, "cd ."), ws.join("src"));
        assert_eq!(manager.cwd(&ws), ws.join("src"));
    }

    #[test]
    fn a_stale_session_cwd_falls_back_to_the_workspace() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-stale");
        std::fs::create_dir_all(ws.join("gone")).unwrap();
        assert_eq!(manager.resolve_cwd(&ws, "cd gone"), ws.join("gone"));
        std::fs::remove_dir_all(ws.join("gone")).unwrap();
        // The remembered cwd has vanished; the call must run from the
        // workspace instead of failing before spawn, and the session state
        // is repaired rather than keeping the dead path.
        assert_eq!(manager.resolve_cwd(&ws, "echo hi"), ws);
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn pushd_advances_like_cd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("pushd");
        std::fs::create_dir_all(ws.join("lib")).unwrap();
        let next = manager.resolve_cwd(&ws, "pushd lib");
        assert_eq!(next, ws.join("lib"));
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
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        // Advance via a leading cd, then read the session cwd back.
        let _ = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "cd src"})),
            )
            .await
            .unwrap();
        assert_eq!(session.cwd(&dir), dir.join("src"));
        // The second call starts from the advanced cwd; a bare command keeps it.
        let _ = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "echo hi"})),
            )
            .await
            .unwrap();
        assert_eq!(session.cwd(&dir), dir.join("src"));
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
