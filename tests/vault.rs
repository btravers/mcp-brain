use mcp_brain::{
    note::{Note, new_ai_note, revision},
    review::Move,
    vault::Vault,
};
use serde_json::json;
use std::{fs, path::Path};
use tempfile::TempDir;

fn fixture() -> (TempDir, Vault) {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("vault")).unwrap();
    let v = Vault::open(temp.path().join("vault"), Some(temp.path().join("state"))).unwrap();
    (temp, v)
}
fn put(v: &Vault, path: &str, text: &str) {
    let path = v.root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn move_to(v: &Vault, source: &str, destination: &str) -> Move {
    Move {
        source: source.into(),
        destination: destination.into(),
        expected_revision: revision(&v.read_text(source).unwrap()),
        reason: "Reference for future learning; no current obligation".into(),
    }
}
fn plan_id(value: &serde_json::Value) -> &str {
    value["id"].as_str().unwrap()
}

#[test]
fn all_notes_search_and_para_pagination_work() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/Alpha.md", "# Alpha\nRust ownership");
    put(&v, "3 Resources/Beta.md", "# Beta\nOther");
    put(&v, "Root.md", "Standalone root note");
    put(&v, ".hidden/Secret.md", "Hidden");
    put(&v, "AGENTS.md", "Vault instructions");
    let first = v.list(None, None, 0, 1).unwrap();
    assert_eq!(first["total"], 3);
    assert_eq!(first["next_offset"], 1);
    assert_eq!(
        v.list(Some("inbox"), Some("RUST"), 0, 50).unwrap()["total"],
        1
    );
    assert!(v.list(Some("bogus"), None, 0, 50).is_err());
    assert_eq!(v.context().unwrap()["vault_rules"], "Vault instructions");
}

#[test]
fn invalid_paths_and_instruction_edits_are_blocked() {
    let (_temp, v) = fixture();
    for path in [
        "../outside.md",
        "/tmp/outside.md",
        "0 Inbox/../../outside.md",
        "0 Inbox\\outside.md",
        ".git/config.md",
        "0 Inbox/NUL.md",
        "0 Inbox/A.md:stream",
        "0 Inbox/./A.md",
        "AGENTS.md",
        "CLAUDE.md",
    ] {
        assert!(v.note_path(path).is_err(), "{path}");
    }
}

#[test]
fn new_notes_have_ai_ownership_and_second_note_builds_index() {
    let (_temp, v) = fixture();
    v.create("First.md", "First", "AI memory").unwrap();
    assert!(!v.root.join("0 Inbox/0 Inbox.md").exists());
    v.create("Second.md", "Second", "Another memory").unwrap();
    assert!(
        v.read_text("0 Inbox/0 Inbox.md")
            .unwrap()
            .contains("[[0 Inbox/First]]")
    );
    assert!(
        v.read_text("0 Inbox/First.md")
            .unwrap()
            .contains("up: \"[[0 Inbox]]\"")
    );
    assert_eq!(v.inbox_status().unwrap()["count"], 2);
    assert!(v.create("First.md", "Overwrite", "No").is_err());
    assert!(v.create("first.md", "Case collision", "No").is_err());
}

#[test]
fn human_mutation_and_stale_edit_are_rejected_without_changes() {
    let (_temp, v) = fixture();
    let path = "0 Inbox/Human.md";
    let original = "# My words\nDo not replace.";
    put(&v, path, original);
    assert!(
        v.edit(path, &revision(original), "Overwrite", "body")
            .is_err()
    );
    assert!(v.edit(path, &revision("Old"), "Comment", "append").is_err());
    assert_eq!(v.read_text(path).unwrap(), original);
    v.edit(path, &revision(original), "Suggestion", "append")
        .unwrap();
    assert!(v.read_text(path).unwrap().starts_with(original));
}

#[test]
fn review_plan_is_non_mutating_then_moves_and_preserves_human_words() {
    let (_temp, v) = fixture();
    let original = "# Gardening\nMy exact words, including typo.";
    put(&v, "0 Inbox/Gardening.md", original);
    let before = v.snapshot().unwrap();
    let plan = v
        .propose_plan(vec![move_to(
            &v,
            "0 Inbox/Gardening.md",
            "3 Resources/Gardening.md",
        )])
        .unwrap();
    assert_eq!(v.snapshot().unwrap(), before);
    let outcome = v.apply_plan(plan_id(&plan)).unwrap();
    assert!(outcome.warning.is_none());
    assert!(!v.root.join("0 Inbox/Gardening.md").exists());
    assert!(
        v.read_text("3 Resources/Gardening.md")
            .unwrap()
            .ends_with(original)
    );
    assert_eq!(v.inbox_status().unwrap()["count"], 0);
    assert!(v.apply_plan(plan_id(&plan)).is_err());
}

#[test]
fn stale_plan_collision_and_duplicate_names_fail_before_moves() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/A.md", "Human A");
    let plan = v
        .propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
        .unwrap();
    put(&v, "0 Inbox/A.md", "Human edited A");
    assert!(v.apply_plan(plan_id(&plan)).is_err());
    assert!(!v.root.join("3 Resources/A.md").exists());
    put(&v, "3 Resources/A.md", "Already exists");
    assert!(
        v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
            .is_err()
    );
}

#[test]
fn generated_indexes_and_ai_parent_links_follow_moves() {
    let (_temp, v) = fixture();
    v.create("A.md", "A", "One").unwrap();
    v.create("B.md", "B", "Two").unwrap();
    let plan = v
        .propose_plan(vec![
            move_to(&v, "0 Inbox/A.md", "3 Resources/A.md"),
            move_to(&v, "0 Inbox/B.md", "3 Resources/B.md"),
        ])
        .unwrap();
    v.apply_plan(plan_id(&plan)).unwrap();
    assert!(
        v.read_text("3 Resources/3 Resources.md")
            .unwrap()
            .contains("[[3 Resources/A]]")
    );
    let moved = v.read_text("3 Resources/A.md").unwrap();
    assert!(moved.contains("[[3 Resources]]"));
    assert!(
        Note::parse(&moved)
            .unwrap()
            .protected_notes_start()
            .unwrap()
            .is_some()
    );
    assert!(
        !v.read_text("0 Inbox/0 Inbox.md")
            .unwrap()
            .contains("[[0 Inbox/A]]")
    );
}

#[test]
fn human_path_links_and_protected_notes_block_unsafe_repairs() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/A.md", "Human A");
    put(
        &v,
        "3 Resources/Ref.md",
        "# Reference\n[[0 Inbox/A|read this]]",
    );
    let before = v.snapshot().unwrap();
    assert!(
        v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
            .is_err()
    );
    assert_eq!(v.snapshot().unwrap(), before);
    put(
        &v,
        "3 Resources/Ref.md",
        &format!(
            "{}[[0 Inbox/A]]\n",
            new_ai_note("Reference", "Body").unwrap()
        ),
    );
    assert!(
        v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
            .is_err()
    );
}

#[test]
fn existing_human_up_is_never_rewritten() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/A.md", "---\nup: '[[0 Inbox]]'\n---\nHuman");
    assert!(
        v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
            .is_err()
    );
}

#[test]
fn reminder_opt_in_disable_due_and_review_lifecycle() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/A.md", "Capture");
    let status = v.inbox_status().unwrap();
    assert_eq!(status["suggest_configuring_reminder"], true);
    assert_eq!(status["due"], false);
    v.configure_reminder(Some(7)).unwrap();
    assert_eq!(v.inbox_status().unwrap()["due"], false);
    v.write_state(
        "reminder.json",
        &json!({"every_days":7,"configured_at":"2020-01-01T00:00:00Z","last_reviewed":null})
            .to_string(),
    )
    .unwrap();
    assert_eq!(v.inbox_status().unwrap()["due"], true);
    v.mark_reviewed().unwrap();
    assert_eq!(v.inbox_status().unwrap()["due"], false);
    v.configure_reminder(None).unwrap();
    assert_eq!(
        v.inbox_status().unwrap()["suggest_configuring_reminder"],
        false
    );
    assert!(v.configure_reminder(Some(0)).is_err());
}

#[test]
fn shared_state_lock_prevents_parallel_mutations() {
    let (_temp, v) = fixture();
    let lock = v.lock().unwrap();
    assert!(v.create("A.md", "A", "Body").is_err());
    drop(lock);
    v.create("A.md", "A", "Body").unwrap();
}

#[test]
fn state_cannot_live_in_the_vault() {
    let temp = TempDir::new().unwrap();
    assert!(Vault::open(temp.path(), Some(temp.path().join("state"))).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_and_hardlink_reads_and_writes_cannot_escape_vault() {
    use std::os::unix::fs::symlink;
    let (temp, v) = fixture();
    let outside = temp.path().join("outside.md");
    fs::write(&outside, "Keep outside").unwrap();
    fs::create_dir(v.root.join("0 Inbox")).unwrap();
    symlink(&outside, v.root.join("0 Inbox/Link.md")).unwrap();
    assert!(v.read_text("0 Inbox/Link.md").is_err());
    assert!(v.create("Link.md", "Link", "Overwrite").is_err());
    fs::hard_link(&outside, v.root.join("0 Inbox/Hard.md")).unwrap();
    assert!(v.read_text("0 Inbox/Hard.md").is_err());
    symlink(temp.path(), v.root.join("3 Resources")).unwrap();
    assert!(v.note_path("3 Resources/outside.md").is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "Keep outside");
}

#[test]
fn cli_requires_a_terminal_for_applying_a_plan() {
    let (_temp, v) = fixture();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"))
        .args([
            "--vault",
            v.root.to_str().unwrap(),
            "--state-dir",
            v.state.to_str().unwrap(),
            "inbox",
            "apply",
            "00000000-0000-0000-0000-000000000000",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));
}

#[test]
fn calendar_is_private_and_has_alarm_and_recurrence() {
    let (_temp, v) = fixture();
    v.configure_reminder(Some(7)).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"))
        .args([
            "--vault",
            v.root.to_str().unwrap(),
            "--state-dir",
            v.state.to_str().unwrap(),
            "reminder",
            "calendar",
            "--time",
            "09:30",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("RRULE:FREQ=DAILY;INTERVAL=7\r\n"));
    assert!(text.contains("BEGIN:VALARM\r\n"));
    assert!(!text.contains(v.root.to_str().unwrap()));
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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

#[test]
fn git_checkpoint_separates_human_and_ai_changes_and_pushes_both() {
    let (temp, v) = fixture();
    let remote = temp.path().join("backup.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(&v.root, &["init", "-b", "main"]);
    git(&v.root, &["config", "user.name", "Example Author"]);
    git(&v.root, &["config", "user.email", "author@example.invalid"]);
    git(&v.root, &["config", "commit.gpgsign", "false"]);
    git(&v.root, &["config", "core.autocrlf", "false"]);
    put(&v, "0 Inbox/Human.md", "Initial human text");
    git(&v.root, &["add", "."]);
    git(&v.root, &["commit", "-m", "Initial notes"]);
    git(
        &v.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&v.root, &["push", "-u", "origin", "main"]);
    put(&v, "0 Inbox/Human.md", "Human edit before MCP");
    v.create("AI.md", "AI", "Invented test memory").unwrap();
    let log = git(&v.root, &["log", "-3", "--format=%s"]);
    assert!(log.starts_with("ai: capture note\nSave existing vault changes\nInitial notes"));
    assert_eq!(
        git(&v.root, &["rev-parse", "HEAD"]),
        git(&v.root, &["rev-parse", "origin/main"])
    );
    assert!(
        v.read_text("0 Inbox/Human.md")
            .unwrap()
            .ends_with("Human edit before MCP")
    );
}

#[test]
fn pending_recovery_journal_blocks_further_note_writes() {
    let (_temp, v) = fixture();
    v.write_state("transaction-example.json", "[]").unwrap();
    assert!(v.create("A.md", "A", "Body").is_err());
    assert!(v.paths().unwrap().is_empty());
    assert!(v.inbox_status().is_ok());
}

#[test]
fn expired_plans_and_non_inbox_sources_are_rejected() {
    let (_temp, v) = fixture();
    put(&v, "0 Inbox/A.md", "Human A");
    let plan = v
        .propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
        .unwrap();
    let id = plan_id(&plan);
    let mut stored = v.load_plan(id).unwrap();
    stored.created = chrono::Utc::now() - chrono::Duration::hours(25);
    v.write_state(
        &format!("plan-{id}.json"),
        &serde_json::to_string(&stored).unwrap(),
    )
    .unwrap();
    assert!(v.apply_plan(id).is_err());
    put(&v, "3 Resources/B.md", "Human B");
    assert!(
        v.propose_plan(vec![move_to(&v, "3 Resources/B.md", "4 Archive/B.md")])
            .is_err()
    );
    assert_eq!(v.read_text("0 Inbox/A.md").unwrap(), "Human A");
}

#[test]
fn unsupported_relative_links_block_moves_before_any_note_changes() {
    for text in [
        "[link](../0%20Inbox/A.md)",
        "[[../0 Inbox/A]]",
        "[[Inbox/A|alias]]",
    ] {
        let (_temp, v) = fixture();
        put(&v, "0 Inbox/A.md", "Capture");
        put(&v, "3 Resources/Reference.md", text);
        assert!(
            v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
                .is_err(),
            "{text}"
        );
    }
    for text in ["[other](Other.md)", "![picture](image.png)", "[[../Other]]"] {
        let (_temp, v) = fixture();
        put(&v, "0 Inbox/A.md", text);
        assert!(
            v.propose_plan(vec![move_to(&v, "0 Inbox/A.md", "3 Resources/A.md")])
                .is_err(),
            "{text}"
        );
    }
}

#[test]
fn git_without_upstream_prevents_any_note_mutation() {
    let (_temp, v) = fixture();
    git(&v.root, &["init", "-b", "main"]);
    assert!(v.create("A.md", "A", "Body").is_err());
    assert!(v.paths().unwrap().is_empty());
}
