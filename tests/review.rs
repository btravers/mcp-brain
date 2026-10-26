use mcp_brain::{
    note::revision,
    review::{Decision, Move},
    vault::Vault,
};
use std::fs;

fn fixture() -> (tempfile::TempDir, Vault, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir_all(root.join("0 Inbox")).unwrap();
    fs::write(
        root.join("0 Inbox/Herbs.md"),
        "# Herbs\nMy original words.\n",
    )
    .unwrap();
    fs::write(root.join("0 Inbox/Trees.md"), "# Trees\nMy other words.\n").unwrap();
    let vault = Vault::open(&root, Some(temp.path().join("state"))).unwrap();
    let moves = ["Herbs", "Trees"].map(|name| {
        let source = format!("0 Inbox/{name}.md");
        Move {
            expected_revision: revision(&vault.read_text(&source).unwrap()),
            source,
            destination: format!("3 Resources/Gardening/{name}.md"),
            reason: "Reference for a future garden".into(),
        }
    });
    let proposal = vault.propose_plan(moves.into()).unwrap();
    let id = proposal["id"].as_str().unwrap().to_owned();
    (temp, vault, id)
}

fn apply_item(vault: &Vault, id: &str, index: usize) {
    let preview = vault.review_item_plan(id, index).unwrap();
    vault.apply_review_item(id, index, &preview.id).unwrap();
}

#[test]
fn review_feedback_survives_restart_without_changing_notes() {
    let (_temp, vault, id) = fixture();
    let before = vault.snapshot().unwrap();
    vault
        .record_review(&id, 0, Decision::Dropped, None)
        .unwrap();
    vault
        .record_review(
            &id,
            1,
            Decision::Commented,
            Some("This is an ongoing responsibility; suggest an Area.".into()),
        )
        .unwrap();
    let reopened = Vault::open(&vault.root, Some(vault.state.clone())).unwrap();
    let feedback = reopened.inbox_plan_review(&id).unwrap();
    assert_eq!(feedback["items"][0]["decision"], "dropped");
    assert_eq!(feedback["items"][1]["decision"], "commented");
    assert!(
        feedback["items"][1]["comment"]
            .as_str()
            .unwrap()
            .contains("Area")
    );
    assert_eq!(reopened.snapshot().unwrap(), before);
    assert!(reopened.prepare_review_plan(&id).is_err());
}

#[test]
fn accepted_subset_rebuilds_indexes_and_preserves_dropped_human_text() {
    let (_temp, vault, id) = fixture();
    let dropped = vault.read_text("0 Inbox/Trees.md").unwrap();
    vault
        .record_review(&id, 0, Decision::Accepted, None)
        .unwrap();
    vault
        .record_review(&id, 1, Decision::Dropped, None)
        .unwrap();
    let original = vault.load_plan(&id).unwrap();
    assert!(
        original
            .changes
            .iter()
            .any(|c| c.path.ends_with("Gardening/Gardening.md"))
    );
    let selected = vault.prepare_review_plan(&id).unwrap();
    assert_eq!(selected.moves.len(), 1);
    assert!(
        !selected
            .changes
            .iter()
            .any(|c| c.path.ends_with("Gardening/Gardening.md"))
    );
    vault.apply_plan(&selected.id).unwrap();
    assert_eq!(vault.read_text("0 Inbox/Trees.md").unwrap(), dropped);
    assert!(
        vault
            .read_text("3 Resources/Gardening/Herbs.md")
            .unwrap()
            .ends_with("# Herbs\nMy original words.\n")
    );
    assert!(!vault.root.join("3 Resources/Gardening/Trees.md").exists());
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied"], true);
    assert!(
        vault
            .record_review(&id, 0, Decision::Accepted, None)
            .is_err()
    );
    assert!(vault.prepare_review_plan(&id).is_err());
}

#[test]
fn partial_review_cannot_refresh_a_stale_snapshot_into_approval() {
    let (_temp, vault, id) = fixture();
    vault
        .record_review(&id, 0, Decision::Accepted, None)
        .unwrap();
    fs::create_dir(vault.root.join("2 Areas")).unwrap();
    fs::write(
        vault.root.join("2 Areas/Responsibilities.md"),
        "A new human note",
    )
    .unwrap();
    let before = vault.snapshot().unwrap();
    assert!(
        vault
            .prepare_review_plan(&id)
            .unwrap_err()
            .to_string()
            .contains("vault changed")
    );
    assert_eq!(vault.snapshot().unwrap(), before);
    assert!(!vault.root.join("3 Resources/Gardening/Herbs.md").exists());
}

#[test]
fn review_comments_are_bounded_and_expired_feedback_remains_readable() {
    let (_temp, vault, id) = fixture();
    assert!(
        vault
            .record_review(&id, 0, Decision::Commented, Some(" ".into()))
            .is_err()
    );
    assert!(
        vault
            .record_review(&id, 0, Decision::Commented, Some("x".repeat(4001)))
            .is_err()
    );
    assert!(
        vault
            .record_review(&id, 2, Decision::Dropped, None)
            .is_err()
    );
    vault
        .record_review(&id, 0, Decision::Accepted, None)
        .unwrap();
    let mut plan = vault.load_plan(&id).unwrap();
    plan.created -= chrono::Duration::days(2);
    vault
        .write_state(
            &format!("plan-{id}.json"),
            &serde_json::to_string(&plan).unwrap(),
        )
        .unwrap();
    assert_eq!(
        vault.inbox_plan_review(&id).unwrap()["items"][0]["decision"],
        "accepted"
    );
    assert!(
        vault
            .prepare_review_plan(&id)
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
}

#[test]
fn cli_defaults_to_human_output_and_json_is_explicit_and_uncolored() {
    let (_temp, vault, id) = fixture();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"))
            .arg("--vault")
            .arg(&vault.root)
            .arg("--state-dir")
            .arg(&vault.state)
            .args(args)
            .output()
            .unwrap()
    };
    let human = run(&["inbox", "show", &id]);
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("Inbox move proposal") && text.contains("Destination"));
    assert!(!text.contains("\"snapshot\"") && !text.contains('\u{1b}'));
    let json = run(&["inbox", "show", &id, "--json", "--color", "always"]);
    assert!(json.status.success());
    let data: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(data["id"], id);
    assert_eq!(data["moves"].as_array().unwrap().len(), 2);
    assert!(json.stderr.is_empty());
    let piped_review = run(&["inbox", "review", &id]);
    assert!(!piped_review.status.success());
    assert!(String::from_utf8_lossy(&piped_review.stderr).contains("interactive terminal"));
    let feedback = run(&["inbox", "feedback", &id, "--json"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&feedback.stdout).unwrap()["items"][0]["decision"],
        "pending"
    );
    let status = run(&["--json", "inbox", "status"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["count"],
        2
    );
}

#[test]
fn direct_application_continues_the_same_review_after_restart() {
    let (_temp, vault, id) = fixture();
    let first = vault.review_item_plan(&id, 0).unwrap();
    vault.apply_review_item(&id, 0, &first.id).unwrap();
    assert_eq!(
        vault.inbox_plan_review(&id).unwrap()["items"][0]["decision"],
        "applied"
    );
    let reopened = Vault::open(&vault.root, Some(vault.state.clone())).unwrap();
    let second = reopened.review_item_plan(&id, 1).unwrap();
    reopened.apply_review_item(&id, 1, &second.id).unwrap();
    let feedback = reopened.inbox_plan_review(&id).unwrap();
    assert_eq!(feedback["applied_count"], 2);
    assert!(
        reopened
            .read_text("3 Resources/Gardening/Herbs.md")
            .unwrap()
            .ends_with("# Herbs\nMy original words.\n")
    );
    assert!(
        reopened
            .read_text("3 Resources/Gardening/Trees.md")
            .unwrap()
            .ends_with("# Trees\nMy other words.\n")
    );
    assert!(reopened.apply_review_item(&id, 0, &first.id).is_err());
}

#[test]
fn direct_apply_does_not_approve_an_external_edit_or_a_foreign_preview() {
    let (_temp, vault, id) = fixture();
    let first = vault.review_item_plan(&id, 0).unwrap();
    let foreign = vault.review_item_plan(&id, 1).unwrap();
    assert!(vault.apply_review_item(&id, 0, &foreign.id).is_err());
    vault.apply_review_item(&id, 0, &first.id).unwrap();
    let second = vault.review_item_plan(&id, 1).unwrap();
    fs::write(vault.root.join("0 Inbox/Trees.md"), "A later human edit").unwrap();
    assert!(vault.apply_review_item(&id, 1, &second.id).is_err());
    assert!(vault.review_item_plan(&id, 1).is_err());
    assert_eq!(
        vault.read_text("0 Inbox/Trees.md").unwrap(),
        "A later human edit"
    );
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 1);
}

#[test]
fn a_receipt_recovers_the_review_when_saving_the_final_ui_state_was_interrupted() {
    let (_temp, vault, id) = fixture();
    let mut intent = vault.review_session(&id).unwrap();
    let first = vault.review_item_plan(&id, 0).unwrap();
    intent.pending_item = Some(mcp_brain::review::ItemApplication {
        index: 0,
        plan_id: first.id.clone(),
    });
    vault.apply_review_item(&id, 0, &first.id).unwrap();
    // Restore the exact intent that was durable before note application.
    vault
        .write_state(
            &format!("review-{id}.json"),
            &serde_json::to_string(&intent).unwrap(),
        )
        .unwrap();
    let recovered = vault.review_session(&id).unwrap();
    assert_eq!(recovered.items[0].decision, Decision::Applied);
    assert!(recovered.pending_item.is_none());
    assert!(vault.apply_review_item(&id, 0, &first.id).is_err());
    let second = vault.review_item_plan(&id, 1).unwrap();
    vault.apply_review_item(&id, 1, &second.id).unwrap();
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 2);
}

#[test]
fn continuation_accounts_for_our_link_and_metadata_changes_without_touching_human_notes() {
    let (_temp, vault, _) = fixture();
    for name in ["A", "B", "C"] {
        let text = format!(
            "---\nauthor: ai\n---\n# {name}\n\nSee [[0 Inbox/A]].\n\n## Notes\n\nHuman suffix for {name}.\n"
        );
        fs::write(vault.root.join(format!("0 Inbox/{name}.md")), text).unwrap();
    }
    let moves: Vec<_> = ["A", "B", "C"]
        .map(|name| {
            let source = format!("0 Inbox/{name}.md");
            Move {
                expected_revision: revision(&vault.read_text(&source).unwrap()),
                source,
                destination: format!("3 Resources/Lessons/{name}.md"),
                reason: "Reusable reference".into(),
            }
        })
        .into();
    // Links in protected Notes are deliberately plain prose, so repair only reaches AI text.
    let plan = vault.propose_plan(moves).unwrap();
    let id = plan["id"].as_str().unwrap();
    let first = vault.review_item_plan(id, 0).unwrap();
    vault.apply_review_item(id, 0, &first.id).unwrap();
    let second = vault.review_item_plan(id, 1).unwrap();
    assert_ne!(
        second.moves[0].expected_revision,
        vault.load_plan(id).unwrap().moves[1].expected_revision
    );
    vault.apply_review_item(id, 1, &second.id).unwrap();
    assert!(
        vault
            .read_text("3 Resources/Lessons/B.md")
            .unwrap()
            .contains("[[3 Resources/Lessons/A]]")
    );
    assert!(
        vault
            .read_text("3 Resources/Lessons/B.md")
            .unwrap()
            .ends_with("## Notes\n\nHuman suffix for B.\n")
    );
}

#[test]
fn undo_restores_exact_human_text_comments_and_review_continuation() {
    let (_temp, vault, id) = fixture();
    vault
        .record_review(
            &id,
            0,
            Decision::Commented,
            Some("This may belong to my ongoing garden responsibility.".into()),
        )
        .unwrap();
    let before = vault.snapshot().unwrap();
    apply_item(&vault, &id, 0);
    let undo = vault.review_undo_plan(&id).unwrap();
    vault.undo_review_item(&id, &undo.id).unwrap();
    assert_eq!(vault.snapshot().unwrap(), before);
    assert_eq!(
        vault.read_text("0 Inbox/Herbs.md").unwrap(),
        "# Herbs\nMy original words.\n"
    );
    let session = vault.review_session(&id).unwrap();
    assert_eq!(session.items[0].decision, Decision::Commented);
    assert!(
        session.items[0]
            .comment
            .as_deref()
            .unwrap()
            .contains("ongoing")
    );
    assert!(vault.undo_review_item(&id, &undo.id).is_err());
    apply_item(&vault, &id, 0);
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 1);
}

#[test]
fn undo_archives_generated_indexes_and_supports_multiple_reverse_steps() {
    let (_temp, vault, id) = fixture();
    apply_item(&vault, &id, 0);
    apply_item(&vault, &id, 1);
    let generated = vault
        .read_text("3 Resources/Gardening/Gardening.md")
        .unwrap();
    let second = vault.review_undo_plan(&id).unwrap();
    assert_eq!(second.index, 1);
    assert_eq!(second.archived_indexes.len(), 1);
    vault.undo_review_item(&id, &second.id).unwrap();
    let archived = vault.read_text(&second.archived_indexes[0]).unwrap();
    assert!(generated.contains("[[3 Resources/Gardening/Herbs]]"));
    assert!(archived.contains("[[Herbs]]") && archived.contains("[[Trees]]"));
    assert_eq!(
        vault.read_text("0 Inbox/Trees.md").unwrap(),
        "# Trees\nMy other words.\n"
    );
    assert!(
        !vault
            .root
            .join("3 Resources/Gardening/Gardening.md")
            .exists()
    );
    let first = vault.review_undo_plan(&id).unwrap();
    assert_eq!(first.index, 0);
    vault.undo_review_item(&id, &first.id).unwrap();
    assert_eq!(
        vault.read_text("0 Inbox/Herbs.md").unwrap(),
        "# Herbs\nMy original words.\n"
    );
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 0);
    assert!(vault.review_undo_plan(&id).is_err());
    apply_item(&vault, &id, 1);
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 1);
    apply_item(&vault, &id, 0);
    assert_eq!(vault.inbox_plan_review(&id).unwrap()["applied_count"], 2);
    assert!(
        vault
            .read_text("3 Resources/Gardening/Gardening.md")
            .is_ok()
    );
    assert!(!second.archived_indexes[0].ends_with("/Gardening.md"));
}

#[test]
fn undo_refuses_to_overwrite_later_human_edits() {
    let (_temp, vault, id) = fixture();
    apply_item(&vault, &id, 0);
    let undo = vault.review_undo_plan(&id).unwrap();
    fs::write(
        vault.root.join("3 Resources/Gardening/Herbs.md"),
        "# Herbs\nLater human edits must survive.\n",
    )
    .unwrap();
    let before = vault.snapshot().unwrap();
    assert!(
        vault
            .undo_review_item(&id, &undo.id)
            .unwrap_err()
            .to_string()
            .contains("later edits")
    );
    assert_eq!(vault.snapshot().unwrap(), before);
    assert!(vault.review_undo_plan(&id).is_err());
}

#[test]
fn undo_also_refuses_an_unrelated_change_or_a_different_review() {
    let (_temp, vault, id) = fixture();
    apply_item(&vault, &id, 0);
    let undo = vault.review_undo_plan(&id).unwrap();
    fs::write(
        vault.root.join("0 Inbox/New thought.md"),
        "A new note outside the review.",
    )
    .unwrap();
    let before = vault.snapshot().unwrap();
    assert!(vault.undo_review_item(&id, &undo.id).is_err());
    assert_eq!(vault.snapshot().unwrap(), before);
    let other = vault
        .propose_plan(vec![Move {
            source: "0 Inbox/Trees.md".into(),
            destination: "3 Resources/Trees.md".into(),
            expected_revision: revision(&vault.read_text("0 Inbox/Trees.md").unwrap()),
            reason: "A reference".into(),
        }])
        .unwrap();
    assert!(
        vault
            .undo_review_item(other["id"].as_str().unwrap(), &undo.id)
            .is_err()
    );
    assert_eq!(vault.snapshot().unwrap(), before);
}

#[test]
fn undo_receipt_recovers_an_interruption_before_the_final_review_save() {
    let (_temp, vault, id) = fixture();
    apply_item(&vault, &id, 0);
    let undo = vault.review_undo_plan(&id).unwrap();
    let mut intent = vault.review_session(&id).unwrap();
    intent.pending_undo_id = Some(undo.id.clone());
    vault.undo_review_item(&id, &undo.id).unwrap();
    vault
        .write_state(
            &format!("review-{id}.json"),
            &serde_json::to_string(&intent).unwrap(),
        )
        .unwrap();
    let recovered = vault.review_session(&id).unwrap();
    assert_eq!(recovered.items[0].decision, Decision::Pending);
    assert!(recovered.applied_items.is_empty());
    assert!(recovered.pending_undo_id.is_none());
    assert!(vault.undo_review_item(&id, &undo.id).is_err());
    apply_item(&vault, &id, 1);
}

#[test]
fn plan_listing_hides_internal_previews_and_reports_progress_and_freshness() {
    let (_temp, vault, id) = fixture();
    let before = vault.snapshot().unwrap();
    let preview = vault.review_item_plan(&id, 0).unwrap();
    assert_eq!(preview.parent_plan_id.as_deref(), Some(id.as_str()));
    let listing = vault.inbox_plans(50).unwrap();
    assert_eq!(listing.plans.len(), 1);
    assert_eq!(listing.plans[0].id, id);
    assert_eq!(listing.plans[0].status, "Ready");
    assert!(listing.plans[0].names.contains(&"Herbs".into()));
    assert_eq!(vault.snapshot().unwrap(), before);
    vault.apply_review_item(&id, 0, &preview.id).unwrap();
    let progress = vault.inbox_plans(50).unwrap();
    assert_eq!(progress.plans.len(), 1);
    assert_eq!(progress.plans[0].applied, 1);
    assert_eq!(progress.plans[0].status, "Ready");
    fs::write(
        vault.root.join("0 Inbox/Trees.md"),
        "Changed outside review",
    )
    .unwrap();
    assert_eq!(vault.inbox_plans(50).unwrap().plans[0].status, "Stale");
}

#[test]
fn plan_listing_keeps_completed_proposals_and_survives_a_corrupt_plan() {
    let (_temp, vault, id) = fixture();
    vault.apply_plan(&id).unwrap();
    let bad_id = uuid::Uuid::new_v4().to_string();
    vault
        .write_state(&format!("plan-{bad_id}.json"), "not JSON")
        .unwrap();
    let listing = vault.inbox_plans(50).unwrap();
    assert_eq!(listing.plans.len(), 1);
    assert_eq!(listing.plans[0].id, id);
    assert_eq!(listing.plans[0].status, "Complete");
    assert_eq!(listing.warnings.len(), 1);
}

#[test]
fn cli_plans_supports_explicit_json_and_undo_requires_a_terminal() {
    let (_temp, vault, id) = fixture();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"))
            .arg("--vault")
            .arg(&vault.root)
            .arg("--state-dir")
            .arg(&vault.state)
            .args(args)
            .output()
            .unwrap()
    };
    let listed = run(&["inbox", "plans", "--json"]);
    assert!(listed.status.success());
    let data: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(data["plans"][0]["id"], id);
    let human = run(&["inbox", "plans"]);
    assert!(String::from_utf8_lossy(&human.stdout).contains("Recent Inbox plans"));
    let undo = run(&["inbox", "undo", &id]);
    assert!(!undo.status.success());
    assert!(String::from_utf8_lossy(&undo.stderr).contains("interactive terminal"));
}
