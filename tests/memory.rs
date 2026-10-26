use mcp_brain::{
    memory::{ArchiveMemory, Basis, MemoryType, Recall, Remember, UpdateMemory},
    note::revision,
    review::Move,
    vault::Vault,
};
use serde_json::Value;
use std::fs;

fn fixture() -> (tempfile::TempDir, Vault) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    let v = Vault::open(root, Some(temp.path().join("state"))).unwrap();
    (temp, v)
}
fn remember(content: &str, scope: Option<&str>) -> Remember {
    Remember {
        title: "Example preference".into(),
        content: content.into(),
        memory_type: MemoryType::Preference,
        scope: scope.map(str::to_owned),
        source: "The user stated this in an invented test conversation.".into(),
        basis: Basis::UserStated,
        review_after: None,
        source_notes: None,
    }
}
fn recall(query: Option<&str>, scope: Option<&str>, archived: bool) -> Recall {
    Recall {
        query: query.map(str::to_owned),
        scope: scope.map(str::to_owned),
        memory_type: None,
        include_archived: archived,
        offset: None,
        limit: None,
        max_chars: None,
    }
}
fn update(memory: &Value, content: &str) -> UpdateMemory {
    UpdateMemory {
        id: memory["id"].as_str().unwrap().into(),
        expected_revision: memory["revision"].as_str().unwrap().into(),
        content: content.into(),
        source: "User explicitly corrected the earlier statement.".into(),
        basis: Basis::UserStated,
        reason: "The preference changed.".into(),
        review_after: None,
        source_notes: None,
        reactivate: false,
    }
}

#[test]
fn default_retrieval_keeps_projects_separate() {
    let (_temp, v) = fixture();
    v.remember(remember("Prefer concise answers.", None))
        .unwrap();
    v.remember(remember("Use framework Alpha.", Some("project:alpha")))
        .unwrap();
    v.remember(remember("Use framework Beta.", Some("project:beta")))
        .unwrap();
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 1);
    assert_eq!(
        v.recall(recall(None, Some("project:alpha"), false))
            .unwrap()["total"],
        2
    );
    assert_eq!(
        v.recall(recall(None, Some("*"), false)).unwrap()["total"],
        3
    );
    assert_eq!(
        v.recall(recall(Some("Beta"), Some("project:alpha"), false))
            .unwrap()["total"],
        0
    );
}

#[test]
fn ordinary_notes_are_not_automatically_memories() {
    let (_temp, v) = fixture();
    v.create(
        "Reference.md",
        "Reference",
        "I might prefer something someday.",
    )
    .unwrap();
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 0);
    assert_eq!(v.list(None, None, 0, 50).unwrap()["total"], 1);
}

#[test]
fn metadata_is_portable_between_providers_and_machines() {
    let (_temp, v) = fixture();
    let saved = v
        .remember(remember("Prefer complete examples.", None))
        .unwrap();
    let memory = &saved["memory"];
    let path = memory["path"].as_str().unwrap();
    let text = v.read_text(path).unwrap();
    assert!(text.contains("kind: memory"));
    let (_other_temp, other) = fixture();
    fs::create_dir_all(other.root.join("0 Inbox")).unwrap();
    fs::write(other.root.join(path), text).unwrap();
    let recalled = other.recall(recall(Some("examples"), None, false)).unwrap();
    assert_eq!(recalled["memories"][0]["memory"]["id"], memory["id"]);
    assert_eq!(
        recalled["memories"][0]["memory"]["source"],
        memory["source"]
    );
}

#[test]
fn updates_preserve_human_annotations_and_history_but_recall_only_current_content() {
    let (_temp, v) = fixture();
    let saved = v.remember(remember("legacytoken", None)).unwrap();
    let path = saved["memory"]["path"].as_str().unwrap();
    let original = v.read_text(path).unwrap();
    fs::write(
        v.root.join(path),
        format!("{original}My exact personal caveat.  \n"),
    )
    .unwrap();
    let current = v.recall(recall(None, None, false)).unwrap()["memories"][0]["memory"].clone();
    let updated = v.update_memory(update(&current, "newtoken")).unwrap();
    let text = v.read_text(path).unwrap();
    assert!(text.ends_with("## Notes\nMy exact personal caveat.  \n"));
    assert!(text.contains("> legacytoken"));
    assert!(text.contains("The preference changed."));
    assert_eq!(
        v.recall(recall(Some("legacytoken"), None, false)).unwrap()["total"],
        0
    );
    assert_eq!(
        v.recall(recall(Some("newtoken"), None, false)).unwrap()["total"],
        1
    );
    assert_eq!(
        updated["memory"]["source"],
        "User explicitly corrected the earlier statement."
    );
    assert!(
        v.update_memory(update(&current, "Stale overwrite"))
            .is_err()
    );
    assert!(
        v.edit(
            path,
            updated["memory"]["revision"].as_str().unwrap(),
            "Erase history",
            "body"
        )
        .is_err()
    );
}

#[test]
fn archive_excludes_memory_without_deleting_it_and_reactivation_is_explicit() {
    let (_temp, v) = fixture();
    let saved = v
        .remember(remember("A temporary preference.", None))
        .unwrap();
    let memory = &saved["memory"];
    let archived = v
        .archive_memory(ArchiveMemory {
            id: memory["id"].as_str().unwrap().into(),
            expected_revision: memory["revision"].as_str().unwrap().into(),
            reason: "No longer relevant.".into(),
        })
        .unwrap();
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 0);
    assert_eq!(v.recall(recall(None, None, true)).unwrap()["total"], 1);
    assert!(v.root.join(memory["path"].as_str().unwrap()).exists());
    assert_eq!(archived["memory"]["archive_reason"], "No longer relevant.");
    let edited = v
        .update_memory(update(
            &archived["memory"],
            "A revised but still inactive preference.",
        ))
        .unwrap();
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 0);
    let mut args = update(&edited["memory"], "A currently valid preference.");
    args.reactivate = true;
    v.update_memory(args).unwrap();
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 1);
}

#[test]
fn review_dates_and_inferred_basis_are_visible_in_recall() {
    let (_temp, v) = fixture();
    let mut args = remember("An uncertain inferred preference.", None);
    args.review_after = Some("2020-01-01".into());
    args.basis = Basis::Inferred;
    v.remember(args).unwrap();
    let memory = &v.recall(recall(None, None, false)).unwrap()["memories"][0]["memory"];
    assert_eq!(memory["review_due"], true);
    assert_eq!(memory["basis"], "inferred");
}

#[test]
fn stable_ids_survive_para_moves_and_archive_folder_hides_active_records() {
    let (_temp, v) = fixture();
    let saved = v
        .remember(remember("A stable memory.", Some("project:demo")))
        .unwrap();
    let memory = &saved["memory"];
    let source = memory["path"].as_str().unwrap();
    let destination = format!("4 Archive/{}", source.rsplit('/').next().unwrap());
    let plan = v
        .propose_plan(vec![Move {
            source: source.into(),
            destination: destination.clone(),
            expected_revision: memory["revision"].as_str().unwrap().into(),
            reason: "The associated work ended.".into(),
        }])
        .unwrap();
    v.apply_plan(plan["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        v.recall(recall(None, Some("project:demo"), false)).unwrap()["total"],
        0
    );
    let archived = v.recall(recall(None, Some("project:demo"), true)).unwrap();
    assert_eq!(archived["memories"][0]["memory"]["id"], memory["id"]);
    assert_eq!(archived["memories"][0]["memory"]["path"], destination);
}

#[test]
fn malformed_and_duplicate_ids_do_not_silently_become_trusted_memory() {
    let (_temp, v) = fixture();
    let saved = v.remember(remember("A memory.", None)).unwrap();
    let path = saved["memory"]["path"].as_str().unwrap();
    let text = v.read_text(path).unwrap();
    fs::write(v.root.join("0 Inbox/Duplicate.md"), &text).unwrap();
    assert!(v.recall(recall(None, None, false)).is_err());
    fs::write(
        v.root.join("0 Inbox/Duplicate.md"),
        "---\nkind: memory\n---\nMalformed record",
    )
    .unwrap();
    let result = v.recall(recall(None, None, false)).unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn invalid_capture_and_duplicate_content_are_rejected() {
    let (_temp, v) = fixture();
    for content in ["", "## Notes\nFake section", "```\nunclosed", "<!-- hidden"] {
        assert!(v.remember(remember(content, None)).is_err(), "{content}");
    }
    assert!(
        v.remember(remember("Preference", Some("project:/home/example")))
            .is_err()
    );
    v.remember(remember("One preference.", None)).unwrap();
    assert!(v.remember(remember("One preference.", None)).is_err());
}

#[test]
fn human_owned_memory_remains_readable_but_cannot_be_rewritten() {
    let (_temp, v) = fixture();
    let saved = v
        .remember(remember("Human now owns this record.", None))
        .unwrap();
    let path = saved["memory"]["path"].as_str().unwrap();
    let text = v
        .read_text(path)
        .unwrap()
        .replace("author: ai", "author: human");
    fs::write(v.root.join(path), &text).unwrap();
    let mut memory = saved["memory"].clone();
    memory["revision"] = revision(&text).into();
    assert!(v.update_memory(update(&memory, "Overwrite")).is_err());
    assert!(
        v.archive_memory(ArchiveMemory {
            id: memory["id"].as_str().unwrap().into(),
            expected_revision: revision(&text),
            reason: "Remove".into()
        })
        .is_err()
    );
    assert_eq!(v.recall(recall(None, None, false)).unwrap()["total"], 1);
    assert_eq!(v.read_text(path).unwrap(), text);
}
