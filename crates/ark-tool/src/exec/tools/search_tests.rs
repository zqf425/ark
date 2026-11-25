//! Unit tests for the builtin `grep` tool. Kept beside the tool instead of
//! inside `search.rs`: the AGENTS.md §4 line cap (1000) is for production
//! code, and the regression suite is the largest single part of this tool.

use super::*;
use crate::exec::common::IGNORED_DIR_NAMES;
use crate::exec::testsupport::{ctx, temp_workspace};

#[tokio::test]
async fn finds_a_line_with_its_position() {
    let dir = temp_workspace("grep-basic");
    std::fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "beta" })),
        )
        .await
        .unwrap();
    assert!(out.ok);
    assert!(out.content.contains("a.txt:2:beta"), "got {}", out.content);
}

#[tokio::test]
async fn match_limit_is_shared_across_files() {
    let dir = temp_workspace("grep-global-limit");
    std::fs::write(dir.join("a.txt"), "needle\nneedle\nneedle\n").unwrap();
    std::fs::write(dir.join("b.txt"), "needle\nneedle\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle", "limit": 2, "context": 1 })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert_eq!(
        out.content
            .lines()
            .filter(|line| line.contains(":needle"))
            .count(),
        2,
        "limit must bound matches across the whole walk: {}",
        out.content
    );
}

#[cfg(windows)]
#[tokio::test]
async fn reached_match_budget_skips_later_named_pipe_and_keeps_context() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    let dir = temp_workspace("grep-budget-pipe");
    let first = dir.join("a.txt");
    std::fs::write(&first, "before\nneedle\nneedle\nafter\n").unwrap();
    let query = Query {
        needle: "needle".into(),
        ignore_case: false,
        file_glob: None,
        limit: 2,
        context: 10,
        matched: Cell::new(0),
    };
    let expected = vec![
        "a.txt-1-before".to_string(),
        "a.txt:2:needle".to_string(),
        "a.txt:3:needle".to_string(),
        "a.txt-4-after".to_string(),
    ];

    let pipe_name = format!(r"\\.\pipe\ark-grep-budget-{}", std::process::id());
    let mut pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    let connected = Arc::new(AtomicBool::new(false));
    let server_connected = Arc::clone(&connected);
    let server = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
            .await
            .is_ok_and(|result| result.is_ok())
        {
            server_connected.store(true, Ordering::SeqCst);
            let _ = pipe.write_all(b"needle\n").await;
        }
    });
    let scan = BlockingScan::start();
    let hits = tokio::time::timeout(
        Duration::from_secs(2),
        scan.run(move |deadline| {
            let mut hits = Vec::new();
            scan_file(&dir, &first, &query, deadline, &mut hits);
            assert_eq!(query.matched.get(), 2);
            assert_eq!(hits, expected, "current-file context must be complete");

            scan_file(&dir, Path::new(&pipe_name), &query, deadline, &mut hits);
            assert_eq!(hits, expected, "existing results must be returned intact");
            Ok(hits)
        }),
    )
    .await
    .expect("a satisfied match budget must not block on a later file")
    .unwrap();
    server.await.unwrap();

    assert!(
        !connected.load(Ordering::SeqCst),
        "a later file must not be opened after the global match budget is reached"
    );
    assert_eq!(hits.len(), 4);
}

#[cfg(windows)]
#[tokio::test]
async fn skips_non_regular_named_pipe_before_open() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    let dir = temp_workspace("grep-non-regular-pipe");
    let query = Query {
        needle: "needle".into(),
        ignore_case: false,
        file_glob: None,
        limit: 2,
        context: 0,
        matched: Cell::new(0),
    };
    let pipe_name = format!(r"\\.\pipe\ark-grep-skip-{}", std::process::id());
    let mut pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    let connected = Arc::new(AtomicBool::new(false));
    let server_connected = Arc::clone(&connected);
    let server = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
            .await
            .is_ok_and(|result| result.is_ok())
        {
            server_connected.store(true, Ordering::SeqCst);
            let _ = pipe.write_all(b"needle\n").await;
        }
    });

    let scan = BlockingScan::start();
    let hits = tokio::time::timeout(
        Duration::from_secs(2),
        scan.run(move |deadline| {
            let mut hits = Vec::new();
            scan_file(&dir, Path::new(&pipe_name), &query, deadline, &mut hits);
            Ok(hits)
        }),
    )
    .await
    .expect("a non-regular file must not block the scan")
    .unwrap();
    server.await.unwrap();

    assert!(
        !connected.load(Ordering::SeqCst),
        "grep must skip a non-regular file before opening it"
    );
    assert!(hits.is_empty());
}

#[cfg(windows)]
#[tokio::test]
async fn explicit_named_pipe_path_is_skipped_before_top_level_metadata() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    let dir = temp_workspace("grep-explicit-pipe");
    let mut context = ctx(&dir);
    context.sandbox = ark_ports::SandboxDecision::Unrestricted;
    let pipe_name = format!(r"\\.\pipe\ark-grep-explicit-{}", std::process::id());
    let mut pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    let connected = Arc::new(AtomicBool::new(false));
    let server_connected = Arc::clone(&connected);
    let server = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
            .await
            .is_ok_and(|result| result.is_ok())
        {
            server_connected.store(true, Ordering::SeqCst);
            let _ = pipe.write_all(b"needle\n").await;
        }
    });

    let output = tokio::time::timeout(
        Duration::from_secs(2),
        run(
            &context,
            ToolArgs(serde_json::json!({ "pattern": "needle", "path": pipe_name })),
        ),
    )
    .await
    .expect("an explicit named pipe must not block the scan")
    .unwrap();
    server.await.unwrap();

    assert!(
        !connected.load(Ordering::SeqCst),
        "grep must reject an explicit named pipe before its top-level metadata lookup"
    );
    assert!(output.ok);
    assert!(output.content.contains("no matches"), "{}", output.content);
}

#[tokio::test]
async fn ignore_case_matches_differently_cased_text() {
    let dir = temp_workspace("grep-case");
    std::fs::write(dir.join("a.txt"), "HELLO\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "hello", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.content.contains("HELLO"));
}

#[tokio::test]
async fn glob_restricts_the_files_scanned() {
    let dir = temp_workspace("grep-glob");
    std::fs::write(dir.join("a.rs"), "needle\n").unwrap();
    std::fs::write(dir.join("b.txt"), "needle\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle", "glob": "*.rs" })),
        )
        .await
        .unwrap();
    assert!(out.content.contains("a.rs"));
    assert!(!out.content.contains("b.txt"));
}

#[tokio::test]
async fn no_match_is_successful_but_explicit() {
    let dir = temp_workspace("grep-none");
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "zzz" })),
        )
        .await
        .unwrap();
    assert!(out.ok);
    assert!(out.content.contains("no matches"));
}

#[tokio::test]
async fn empty_pattern_is_refused() {
    let dir = temp_workspace("grep-empty");
    let out = GrepTool
        .execute(&ctx(&dir), ToolArgs(serde_json::json!({ "pattern": "" })))
        .await
        .unwrap();
    assert!(!out.ok);
}

#[tokio::test]
async fn regex_looking_patterns_are_refused_not_matched_literally() {
    let dir = temp_workspace("grep-regex-refused");
    // The literal sequence `a.c` does not occur; a literal scan would
    // return "no matches", which once read as a fact about the code.
    std::fs::write(dir.join("a.txt"), "abc\na.c\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "a.c" })),
        )
        .await
        .unwrap();
    assert!(!out.ok, "got {}", out.content);
    assert!(
        out.content.contains("metacharacters"),
        "got {}",
        out.content
    );
}

#[tokio::test]
async fn literal_true_searches_needles_that_look_like_regex() {
    // The flag exists exactly for this case. Ignoring it meant a model that
    // already opted into a byte-exact search still had to rewrite the call
    // through `bash` (22 such refusals observed in one day).
    let dir = temp_workspace("grep-literal-accepted");
    std::fs::write(dir.join("a.java"), "int x = getExecType();\n").unwrap();
    for pattern in ["getExecType()", "ExecType()|Other("] {
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": pattern, "literal": true })),
            )
            .await
            .unwrap();
        assert!(
            out.ok,
            "literal search must not be refused: {}",
            out.content
        );
        if pattern == "getExecType()" {
            assert!(out.content.contains("a.java:1:"), "got {}", out.content);
        } else {
            assert!(out.content.contains("no matches"), "got {}", out.content);
        }
    }
}

#[tokio::test]
async fn a_regex_refusal_names_the_literal_flag() {
    let dir = temp_workspace("grep-refusal-message");
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "a.*b" })),
        )
        .await
        .unwrap();
    assert!(!out.ok);
    assert!(
        out.content.contains("literal: true"),
        "the refusal must name the escape hatch: {}",
        out.content
    );
}

#[tokio::test]
async fn alternation_patterns_are_refused() {
    let dir = temp_workspace("grep-regex-alt");
    std::fs::write(dir.join("a.txt"), "threshold\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({
                "pattern": "0\\.8|80%|threshold.*0\\.8"
            })),
        )
        .await
        .unwrap();
    assert!(!out.ok, "got {}", out.content);
}

#[tokio::test]
async fn build_and_cache_directories_are_skipped() {
    let dir = temp_workspace("grep-ignore-dirs");
    // Iterate the real constant: a name added later lands here or the
    // list change ships without coverage.
    for name in IGNORED_DIR_NAMES {
        let sub = dir.join(name);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("gen.js"), "needle-in-generated\n").unwrap();
    }
    std::fs::write(dir.join("src.txt"), "needle-in-source\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle" })),
        )
        .await
        .unwrap();
    assert!(out.content.contains("src.txt"), "got {}", out.content);
    assert!(
        !out.content.contains("needle-in-generated"),
        "generated output must not be scanned: {}",
        out.content
    );
}

#[tokio::test]
async fn an_ignored_directory_can_still_be_searched_explicitly() {
    let dir = temp_workspace("grep-ignore-explicit");
    std::fs::create_dir_all(dir.join("target")).unwrap();
    std::fs::write(dir.join("target/gen.js"), "needle\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle", "path": "target" })),
        )
        .await
        .unwrap();
    // The relative prefix is relative to the explicit search root, so the
    // file shows as `gen.js`; the point is that the ignored directory was
    // still searched instead of silently yielding nothing.
    assert_eq!(out.content, "gen.js:1:needle\n");
}

#[tokio::test]
async fn ignore_case_keeps_the_cut_window_on_the_match_when_folding_expands_chars() {
    let dir = temp_workspace("grep-fold-offset");
    // Case 1: length-changing chars in the LINE, ASCII needle — the
    // per-byte ASCII fast path must anchor the offset on the original
    // line instead of the folded one.
    let line = format!("{}dotted{}", "İ".repeat(300), "z".repeat(500));
    std::fs::write(dir.join("fold.txt"), &line).unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "DOTTED", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("dotted"),
        "the cut window must still contain the match: {}",
        out.content
    );

    // Case 2: length-changing chars in the NEEDLE — full folding runs,
    // and its folded char offset must be remapped onto the original line.
    let line = format!("{}İdotted{}", "İ".repeat(300), "z".repeat(500));
    std::fs::write(dir.join("folded-needle.txt"), &line).unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "İDOTTED", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("dotted"),
        "the cut window must still contain the match: {}",
        out.content
    );
}

/// A needle over 32 bytes falls back to TwoWay for both `contains` and
/// `find`, so the fast path is a single folded `find`. The fold is mandatory:
/// the first draft of this branch searched the raw line and turned every
/// differently-cased line into a false "no matches" (P0 in review).
#[tokio::test]
async fn ignore_case_long_needle_still_folds_case() {
    let dir = temp_workspace("grep-long-needle-fold");
    let needle = "ThisIsAVeryLongAsciiNeedleOver32Bytes"; // 38 bytes
    assert!(needle.len() > 32);
    std::fs::write(
        dir.join("f.txt"),
        "call thisisaverylongasciineedleover32bytes now\n",
    )
    .unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": needle, "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("f.txt:1:"),
        "a >32B ASCII needle must still match case-insensitively: {}",
        out.content
    );
}

/// A decoded NUL byte marks object files, images and other binary blobs:
/// one hit inside them floods the output and reads as a fact about source
/// code. UTF-16LE without a BOM decodes with interleaved NULs, so it is
/// skipped by the same check. The text hit in the same tree must survive.
#[tokio::test]
async fn binary_files_with_decoded_nul_are_skipped() {
    let dir = temp_workspace("grep-nul-skip");
    std::fs::write(dir.join("blob.bin"), b"needle\x00needle\n".as_slice()).unwrap();
    std::fs::write(dir.join("u16.txt"), "n\x00e\x00e\x00d\x00l\x00e\x00").unwrap();
    std::fs::write(dir.join("text.txt"), "needle\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle" })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("text.txt:1:needle"),
        "the text hit must survive: {}",
        out.content
    );
    assert!(
        !out.content.contains("blob.bin") && !out.content.contains("u16.txt"),
        "NUL-carrying files must not be reported: {}",
        out.content
    );
}

/// Non-ASCII needle under `ignore_case`: the full-folding path must not
/// turn "ASCII fold missed" into a hit. The earlier draft wrote
/// `!ascii.contains() ||`, which marked every non-matching line a hit,
/// burned the global limit on them and pinned their cut window at column
/// 0. These tests were green with that bug in place — this case is the
/// only guard for it.
#[tokio::test]
async fn ignore_case_non_ascii_needle_does_not_match_unrelated_lines() {
    let dir = temp_workspace("grep-nonascii-needle");
    std::fs::write(dir.join("u.txt"), "totally unrelated\nthe café is open\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "CAFÉ", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    let matches: Vec<&str> = out
        .content
        .lines()
        .filter(|line| line.contains(':'))
        .collect();
    assert_eq!(
        matches,
        vec!["u.txt:2:the café is open"],
        "only the real match may be reported: {}",
        out.content
    );

    std::fs::write(dir.join("cjk.txt"), "你好世界\n这里需要登录\n再见\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "登录", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert_eq!(
        out.content.lines().count(),
        1,
        "exactly the one matching line: {}",
        out.content
    );
    assert!(out.content.contains("cjk.txt:2:这里需要登录"));
}

/// ASCII folding misses matches that pass through non-ASCII chars folding
/// *into* ASCII (K U+212A → k, İ → i). Deliberate trade-off: the old
/// full-fold behavior found those but also matched whole Turkish lines
/// when searching `i`. Pinned here so a future change is a decision, not
/// an accident.
#[tokio::test]
async fn ignore_case_ascii_path_misses_matches_through_expanding_chars() {
    let dir = temp_workspace("grep-ascii-miss");
    std::fs::write(dir.join("kelvin.txt"), "300 \u{212A} cold\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "K", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert!(out.ok, "{}", out.content);
    assert!(
        !out.content.contains("kelvin.txt"),
        "ASCII-fold matching must not find K via U+212A: {}",
        out.content
    );
}

#[tokio::test]
async fn ignore_case_ascii_fast_path_still_matches_case_variants() {
    let dir = temp_workspace("grep-ascii-fold");
    std::fs::write(
        dir.join("a.txt"),
        "getExecType()\nGETEXECTYPE()\ngetexectype()\n",
    )
    .unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "GetExecType", "ignore_case": true })),
        )
        .await
        .unwrap();
    assert_eq!(out.content.lines().count(), 3, "{}", out.content);
    assert!(out.content.contains("a.txt:1:getExecType()"));
    assert!(out.content.contains("a.txt:2:GETEXECTYPE()"));
    assert!(out.content.contains("a.txt:3:getexectype()"));
}

#[tokio::test]
async fn a_minified_single_line_match_is_cut_to_a_bounded_prefix() {
    let dir = temp_workspace("grep-line-cut");
    let bundle = format!("{}needle{}", "x".repeat(100_000), "y".repeat(100_000));
    std::fs::write(dir.join("bundle.js"), bundle).unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle" })),
        )
        .await
        .unwrap();
    assert!(out.ok);
    assert!(
        out.content.len() < 2_000,
        "one match line must stay bounded, got {} chars",
        out.content.len()
    );
    assert!(out.content.contains("needle"));
}

#[tokio::test]
async fn overlapping_context_windows_emit_each_line_once() {
    let dir = temp_workspace("grep-context-overlap");
    std::fs::write(dir.join("a.txt"), "hit-a\nhit-b\nhit-c\nhit-d\nhit-e\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "hit", "context": 1 })),
        )
        .await
        .unwrap();
    for n in 1..=5 {
        let occurrences = out.content.matches(&format!("a.txt-{n}-")).count()
            + out.content.matches(&format!("a.txt:{n}:")).count();
        assert_eq!(
            occurrences, 1,
            "line {n} must appear exactly once: {}",
            out.content
        );
    }
}

#[tokio::test]
async fn limit_counts_matches_not_context_rows() {
    let dir = temp_workspace("grep-limit-context");
    std::fs::write(dir.join("a.txt"), "m1\nm2\nm3\nm4\nm5\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "m", "limit": 2, "context": 2 })),
        )
        .await
        .unwrap();
    // Exactly two *matching* lines; later lines may still appear as
    // context rows (`-`), which is correct, but never as matches (`:`).
    assert!(out.content.contains("a.txt:1:m1"));
    assert!(out.content.contains("a.txt:2:m2"));
    assert!(
        !out.content.contains("a.txt:3:m3"),
        "a third real match must not be emitted: {}",
        out.content
    );
    assert!(!out.content.contains("a.txt:5:m5"));
}

#[tokio::test]
async fn a_max_usize_context_does_not_panic() {
    let dir = temp_workspace("grep-overflow");
    std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle", "context": usize::MAX })),
        )
        .await
        .unwrap();
    assert!(out.ok);
}

#[cfg(unix)]
#[tokio::test]
async fn grep_does_not_follow_symlinks_out_of_the_workspace() {
    use std::os::unix::fs::symlink;
    let dir = temp_workspace("grep-symlink-escape");
    let outside = temp_workspace("grep-symlink-outside");
    std::fs::write(outside.join("secret.txt"), "needle-secret\n").unwrap();
    symlink(&outside, dir.join("link")).unwrap();
    let out = GrepTool
        .execute(
            &ctx(&dir),
            ToolArgs(serde_json::json!({ "pattern": "needle" })),
        )
        .await
        .unwrap();
    assert!(
        !out.content.contains("needle-secret"),
        "a symlink must not leak external contents: {}",
        out.content
    );
}
