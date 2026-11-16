use mcp_brain::{
    memory::{Basis, MemoryType, Recall, Remember},
    vault::Vault,
};
use std::{fs, path::PathBuf};

fn fixture() -> (tempfile::TempDir, Vault) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    fs::create_dir(&root).unwrap();
    let v = Vault::open(root, Some(temp.path().join("state"))).unwrap();
    (temp, v)
}
fn remember(title: &str, content: &str) -> Remember {
    Remember {
        title: title.into(),
        content: content.into(),
        memory_type: MemoryType::Preference,
        scope: None,
        source: "The user stated this in an invented test conversation.".into(),
        basis: Basis::UserStated,
        review_after: None,
        source_notes: None,
    }
}
fn recall(query: &str) -> Recall {
    Recall {
        query: Some(query.into()),
        scope: None,
        memory_type: None,
        include_archived: false,
        offset: None,
        limit: None,
        max_chars: None,
    }
}

#[test]
fn without_a_model_search_stays_lexical_and_bad_files_never_install() {
    let (temp, v) = fixture();
    fs::write(v.root.join("A.md"), "alpha\n").unwrap();
    let result = v.search_notes("alpha", None, 0, 20).unwrap();
    assert!(
        result["ranking"]
            .as_str()
            .unwrap()
            .contains("model install")
    );
    assert!(result["notes"][0]["similarity"].is_null());
    let fake = temp.path().join("fake");
    fs::create_dir(&fake).unwrap();
    for file in ["config.json", "tokenizer.json", "model.safetensors"] {
        fs::write(fake.join(file), "not the model").unwrap();
    }
    let error = v.install_model(Some(&fake)).unwrap_err().to_string();
    assert!(error.contains("checksum mismatch"), "{error}");
    assert!(!v.state.join("model").exists());
}

/// Needs the real model files; the eprintln output helps recalibrate floors.
/// Run with:
/// MCP_BRAIN_TEST_MODEL_DIR=/path/to/files cargo test --release -- --ignored --nocapture
#[test]
#[ignore = "needs the 490 MB model; set MCP_BRAIN_TEST_MODEL_DIR"]
fn model_finds_meaning_across_languages_and_flags_overlaps() {
    let (_temp, v) = fixture();
    let dir = PathBuf::from(std::env::var("MCP_BRAIN_TEST_MODEL_DIR").unwrap());
    v.install_model(Some(&dir)).unwrap();
    assert_eq!(
        v.install_model(Some(&dir)).unwrap()["already_installed"],
        true
    );
    for folder in ["0 Inbox", "3 Resources"] {
        fs::create_dir(v.root.join(folder)).unwrap();
    }
    let note = |path: &str, text: &str| fs::write(v.root.join(path), text).unwrap();
    note(
        "3 Resources/Hosting.md",
        "# Réunion budget\nNous avons décidé de réduire les coûts d'hébergement des serveurs.\n",
    );
    note(
        "3 Resources/Garden.md",
        "# Garden\nTomatoes need sun and regular watering in summer.\n",
    );
    note(
        "3 Resources/Rust.md",
        "# Rust ownership\nEach value has a single owner; borrowing lends access without moving it.\n",
    );
    note(
        "0 Inbox/Borrow checker.md",
        "# Borrow checker\nThe compiler refuses two mutable references to the same value.\n",
    );
    let rebuilt = v.rebuild_index().unwrap();
    assert!(
        rebuilt["semantic"]
            .as_str()
            .unwrap()
            .contains("All notes embedded")
    );

    // English words, French note: no shared stem, found by meaning.
    let found = v
        .search_notes("server hosting expenses", None, 0, 20)
        .unwrap();
    eprintln!("cross-lingual: {found:#}");
    assert!(found["ranking"].as_str().unwrap().starts_with("Hybrid"));
    assert_eq!(found["notes"][0]["path"], "3 Resources/Hosting.md");
    assert_eq!(found["total"], 1, "unrelated notes stay below the floor");

    v.remember(remember(
        "Answer style",
        "I prefer short answers with runnable code samples.",
    ))
    .unwrap();
    v.remember(remember(
        "Explanations",
        "Keep explanations concise and include examples that run.",
    ))
    .unwrap();
    v.remember(remember("Coffee", "I drink my coffee without sugar."))
        .unwrap();
    let memories = v.recall(recall("how verbose should replies be")).unwrap();
    eprintln!("recall: {memories:#}");
    let titles: Vec<_> = memories["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["memory"]["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, ["Answer style", "Explanations"]);

    let inbox = v.inbox_review(0, 20).unwrap();
    eprintln!("inbox: {inbox:#}");
    let item = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == "0 Inbox/Borrow checker.md")
        .unwrap();
    assert_eq!(item["similar_notes"].as_array().unwrap().len(), 1);
    assert_eq!(item["similar_notes"][0]["path"], "3 Resources/Rust.md");

    let review = v.memory_review(0, 20).unwrap();
    eprintln!("review: {review:#}");
    // Only the two answer-style memories overlap; no shared words or titles.
    assert_eq!(review["total"], 1);
    assert!(review["findings"][0]["semantic_similarity"].is_number());
}
