use mcp_brain::{
    git,
    operations::Policy,
    vault::{Change, Vault},
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn command(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn fixture() -> (tempfile::TempDir, Vault) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    let v = Vault::open(root, Some(temp.path().join("state"))).unwrap();
    (temp, v)
}
fn setup(v: &Vault, remote: &Path) {
    fs::create_dir(remote).unwrap();
    command(remote, &["init", "--bare", "-b", "main"]);
    command(&v.root, &["init", "-b", "main"]);
    identity(&v.root);
    fs::write(v.root.join("Human.md"), "Initial words\n").unwrap();
    command(&v.root, &["add", "."]);
    command(&v.root, &["commit", "-m", "Initial notes"]);
    command(
        &v.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    command(&v.root, &["push", "-u", "origin", "main"]);
}
fn identity(root: &Path) {
    command(root, &["config", "user.name", "Example Author"]);
    command(root, &["config", "user.email", "author@example.invalid"]);
    command(root, &["config", "commit.gpgsign", "false"]);
    command(root, &["config", "core.autocrlf", "false"]);
}
fn remember(v: &Vault, content: &str) -> Value {
    v.remember(serde_json::from_value(json!({"title":"Example preference","content":content,"memory_type":"preference","source":"Invented user statement","basis":"user_stated"})).unwrap()).unwrap()
}

#[test]
fn exact_ai_snapshot_preserves_concurrent_worktree_and_staged_edits() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    let before = "Initial words\n";
    let after = "Initial words\n\n## AI\n<!-- ai-generated, safe to delete -->\nSuggestion\n";
    let human = "Initial words\nHuman typed later\n";
    fs::write(v.root.join("Human.md"), human).unwrap();
    command(&v.root, &["add", "Human.md"]);
    fs::write(v.root.join("Other.md"), "Separately staged human work\n").unwrap();
    command(&v.root, &["add", "Other.md"]);
    git::finish(
        &v.root,
        &[Change {
            path: "Human.md".into(),
            before: Some(before.into()),
            after: Some(after.into()),
        }],
        "test snapshot",
    )
    .unwrap();
    assert_eq!(command(&v.root, &["show", "HEAD:Human.md"]), after);
    assert_eq!(fs::read_to_string(v.root.join("Human.md")).unwrap(), human);
    assert_eq!(command(&v.root, &["show", ":Human.md"]), human);
    assert!(command(&v.root, &["diff", "--cached", "--name-only"]).contains("Other.md"));
    assert!(!command(&v.root, &["ls-tree", "--name-only", "HEAD"]).contains("Other.md"));
}

#[test]
fn exact_commit_refuses_to_replace_a_newer_human_commit() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    fs::write(v.root.join("Human.md"), "Newer human commitment\n").unwrap();
    command(&v.root, &["add", "."]);
    command(&v.root, &["commit", "-m", "Newer human work"]);
    assert!(
        git::finish(
            &v.root,
            &[Change {
                path: "Human.md".into(),
                before: Some("Initial words\n".into()),
                after: Some("Earlier AI proposal\n".into())
            }],
            "recovery"
        )
        .is_err()
    );
    assert_eq!(
        command(&v.root, &["show", "HEAD:Human.md"]),
        "Newer human commitment\n"
    );
}

#[test]
fn commit_failure_retains_journal_and_recovery_finishes_exactly_once() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    let index_lock = v.root.join(".git/index.lock");
    fs::write(&index_lock, "").unwrap();
    let outcome = v
        .create("Capture.md", "Capture", "An invented capture.")
        .unwrap();
    assert!(outcome.warning.unwrap().contains("commit is incomplete"));
    let journal = v.journals().unwrap().remove(0);
    assert!(
        v.create("Blocked.md", "Blocked", "Do not write yet")
            .is_err()
    );
    fs::remove_file(index_lock).unwrap();
    v.recovery_finish(&journal).unwrap();
    assert!(v.journals().unwrap().is_empty());
    let head = command(&v.root, &["rev-parse", "HEAD"]);
    assert_eq!(command(&v.root, &["rev-parse", "origin/main"]), head);
    assert!(v.recovery_finish(&journal).is_err());
    assert_eq!(command(&v.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn line_ending_conversion_is_rejected_before_changing_human_bytes() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    command(&v.root, &["config", "core.autocrlf", "true"]);
    let original = "Initial words\r\n";
    fs::write(v.root.join("Human.md"), original).unwrap();
    let error = v
        .edit(
            "Human.md",
            &mcp_brain::note::revision(original),
            "An AI suggestion",
            "append",
        )
        .unwrap_err();
    assert!(error.to_string().contains("conversion"), "{error:#}");
    assert_eq!(v.read_text("Human.md").unwrap(), original);
    assert!(v.journals().unwrap().is_empty());
}

#[test]
fn offline_opt_in_keeps_local_commits_and_retry_does_not_repeat_edits() {
    let (temp, v) = fixture();
    let remote = temp.path().join("backup.git");
    setup(&v, &remote);
    let unavailable = temp.path().join("unavailable.git");
    fs::rename(&remote, &unavailable).unwrap();
    assert!(
        v.create("Strict.md", "Strict", "Cannot write before backup")
            .is_err()
    );
    assert!(!v.root.join("0 Inbox/Strict.md").exists());
    v.configure(Policy {
        offline_writes: true,
        ..Default::default()
    })
    .unwrap();
    assert!(
        v.create("One.md", "One", "First offline capture")
            .unwrap()
            .warning
            .is_some()
    );
    assert!(v.journals().unwrap().is_empty());
    assert!(
        v.create("Two.md", "Two", "Second offline capture")
            .unwrap()
            .warning
            .is_some()
    );
    assert!(v.sync_status().unwrap()["pending_push"].is_object());
    let head = command(&v.root, &["rev-parse", "HEAD"]);
    fs::rename(&unavailable, &remote).unwrap();
    assert!(v.retry_push().unwrap()["warning"].is_null());
    assert_eq!(command(&v.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(command(&v.root, &["rev-parse", "origin/main"]), head);
    assert!(v.sync_status().unwrap()["pending_push"].is_null());
}

#[test]
fn sync_is_explicit_fast_forward_only_and_refuses_dirty_or_diverged_trees() {
    let (temp, v) = fixture();
    let remote = temp.path().join("backup.git");
    setup(&v, &remote);
    let peer = temp.path().join("peer");
    command(
        temp.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    identity(&peer);
    fs::write(peer.join("Remote.md"), "Remote capture\n").unwrap();
    command(&peer, &["add", "."]);
    command(&peer, &["commit", "-m", "Remote capture"]);
    command(&peer, &["push"]);
    assert_eq!(v.list(None, None, 0, 50).unwrap()["total"], 1);
    fs::write(v.root.join("Human.md"), "Unsaved locally\n").unwrap();
    assert!(v.synchronize().is_err());
    assert_eq!(v.read_text("Human.md").unwrap(), "Unsaved locally\n");
    fs::write(v.root.join("Human.md"), "Initial words\n").unwrap();
    v.synchronize().unwrap();
    assert_eq!(v.read_text("Remote.md").unwrap(), "Remote capture\n");
    assert!(v.sync_status().unwrap()["last_sync"]["completed_at"].is_string());
    fs::write(peer.join("Remote.md"), "Second remote capture\n").unwrap();
    command(&peer, &["add", "."]);
    command(&peer, &["commit", "-m", "Second remote"]);
    command(&peer, &["push"]);
    fs::write(v.root.join("Human.md"), "Local committed edit\n").unwrap();
    command(&v.root, &["add", "."]);
    command(&v.root, &["commit", "-m", "Local edit"]);
    let head = command(&v.root, &["rev-parse", "HEAD"]);
    assert!(
        v.synchronize()
            .unwrap_err()
            .to_string()
            .contains("diverged")
    );
    assert_eq!(command(&v.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn watcher_checkpoints_human_saves_without_ai_prefix_and_ignores_unchanged_state() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    fs::write(v.root.join("Human.md"), "First edit\n").unwrap();
    let first = v.watch_fingerprint().unwrap();
    fs::write(v.root.join("Human.md"), "Second edit\n").unwrap();
    assert_ne!(first, v.watch_fingerprint().unwrap());
    v.watch_once().unwrap();
    assert_eq!(
        command(&v.root, &["log", "-1", "--format=%s"]),
        "Save existing vault changes\n"
    );
    let head = command(&v.root, &["rev-parse", "HEAD"]);
    v.watch_once().unwrap();
    assert_eq!(command(&v.root, &["rev-parse", "HEAD"]), head);
}

#[test]
fn git_coordinator_is_shared_even_with_different_state_locations() {
    let (temp, v) = fixture();
    setup(&v, &temp.path().join("backup.git"));
    let other = Vault::open(&v.root, Some(temp.path().join("other-state"))).unwrap();
    let _lock = v.lock().unwrap();
    assert!(other.lock().is_err());
}

#[test]
fn interrupted_recovery_resumes_but_never_overwrites_later_human_work() {
    let (_temp, v) = fixture();
    let changes = vec![
        Change {
            path: "First.md".into(),
            before: None,
            after: Some("First intended\n".into()),
        },
        Change {
            path: "Second.md".into(),
            before: None,
            after: Some("Second intended\n".into()),
        },
    ];
    v.write_state(
        "transaction-test.json",
        &serde_json::to_string(&changes).unwrap(),
    )
    .unwrap();
    fs::write(
        v.root.join("First.md"),
        "First intended\nHuman annotation\n",
    )
    .unwrap();
    assert!(v.recovery_finish("transaction-test.json").is_err());
    assert!(!v.root.join("Second.md").exists());
    fs::write(v.root.join("First.md"), "First intended\n").unwrap();
    v.recovery_finish("transaction-test.json").unwrap();
    assert_eq!(v.read_text("Second.md").unwrap(), "Second intended\n");
    assert!(v.journals().unwrap().is_empty());
}

#[test]
fn search_index_detects_external_edits_deletes_and_recovers_corruption() {
    let (_temp, v) = fixture();
    fs::write(v.root.join("Example.md"), "# Example\nOldtoken café\n").unwrap();
    assert_eq!(v.search_notes("oldtoken", None, 0, 20).unwrap()["total"], 1);
    fs::write(v.root.join("Example.md"), "# Example\nNewtoken café\n").unwrap();
    assert_eq!(v.search_notes("oldtoken", None, 0, 20).unwrap()["total"], 0);
    assert_eq!(
        v.search_notes("CAFÉ newtoken", None, 0, 20).unwrap()["notes"][0]["score"],
        2
    );
    v.write_state("search-index.json", "corrupt cache").unwrap();
    assert_eq!(v.search_notes("newtoken", None, 0, 20).unwrap()["total"], 1);
    fs::remove_file(v.root.join("Example.md")).unwrap();
    assert_eq!(v.search_notes("newtoken", None, 0, 20).unwrap()["total"], 0);
}

#[test]
fn memory_budget_returns_explicit_references_with_forward_pagination() {
    let (_temp, v) = fixture();
    remember(&v, &"Long sentence. ".repeat(2000));
    remember(&v, &"Another lengthy sentence. ".repeat(2000));
    let result = v
        .recall(serde_json::from_value(json!({"max_chars":2048})).unwrap())
        .unwrap();
    assert!(result["context_chars"].as_u64().unwrap() <= 2048);
    assert_eq!(result["memories"][0]["truncated"], true);
    assert_eq!(result["next_offset"], 1);
    let next = v
        .recall(serde_json::from_value(json!({"max_chars":2048,"offset":1})).unwrap())
        .unwrap();
    assert_ne!(
        result["memories"][0]["memory"]["id"],
        next["memories"][0]["memory"]["id"]
    );
    assert!(next["next_offset"].is_null());
}

#[test]
fn review_tracks_changed_sources_and_returns_proposals_without_edits() {
    let (_temp, v) = fixture();
    fs::write(v.root.join("Source.md"), "Invented source\n").unwrap();
    v.remember(serde_json::from_value(json!({"title":"Example preference","content":"Prefer concise examples.","memory_type":"preference","source":"[[Source]]","source_notes":["Source.md"],"basis":"user_stated","review_after":"2020-01-01"})).unwrap()).unwrap();
    remember(&v, "Prefer detailed examples.");
    fs::write(v.root.join("Source.md"), "Updated invented source\n").unwrap();
    let before = v.snapshot().unwrap();
    let review = v.memory_review(0, 100).unwrap();
    let kinds: Vec<_> = review["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"overdue"));
    assert!(kinds.contains(&"source_changed"));
    assert!(kinds.contains(&"possible_conflict_or_overlap"));
    assert_eq!(before, v.snapshot().unwrap());
}

#[test]
fn init_is_additive_and_doctor_requires_no_remote_connection() {
    let (_temp, v) = fixture();
    fs::write(v.root.join("Human.md"), "Exact human words\n").unwrap();
    let init = v.initialize().unwrap();
    assert!(init["claude_code"]["mcpServers"]["brain"].is_object());
    assert_eq!(v.read_text("Human.md").unwrap(), "Exact human words\n");
    assert_eq!(v.doctor().unwrap()["ok"], true);
}
