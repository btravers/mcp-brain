use mcp_brain::note::{AI_MARKER, Author, Note, new_ai_note, revision};

#[test]
fn missing_unknown_and_nonstring_authors_are_human() {
    for text in [
        "# My note\n",
        "---\nauthor: human\n---\nText",
        "---\nauthor: AI\n---\nText",
        "---\nauthor: [ai]\n---\nText",
        "---\nauthor: true\n---\nText",
    ] {
        let note = Note::parse(text).unwrap();
        assert_eq!(note.author, Author::Human);
        assert!(note.replace_ai_body("Overwrite").is_err());
    }
}

#[test]
fn malformed_or_duplicate_frontmatter_fails_closed() {
    for text in [
        "---\nauthor: ai\nauthor: human\n---\nText",
        "---\nauthor: ai\n",
        "---\n[not a mapping]\n---\nText",
    ] {
        assert!(Note::parse(text).is_err(), "{text}");
    }
}

#[test]
fn comments_preserve_every_human_byte_and_can_be_refined() {
    for original in [
        "# Human\nOriginal text with typo.\n",
        "No final newline",
        "---\r\nauthor: human\r\ncustom: value\r\n---\r\n# Human\r\nOriginal\r\n",
    ] {
        let first = Note::parse(original)
            .unwrap()
            .set_ai_section("First comment", true)
            .unwrap();
        assert!(first.starts_with(original));
        let second = Note::parse(&first)
            .unwrap()
            .set_ai_section("Second comment", true)
            .unwrap();
        assert!(second.starts_with(&first));
        let replacement = Note::parse(&second)
            .unwrap()
            .set_ai_section("Revised comment", false)
            .unwrap();
        assert!(replacement.starts_with(original));
        assert!(!replacement.contains("First comment"));
        assert_eq!(replacement.matches("## AI").count(), 1);
    }
}

#[test]
fn human_ai_heading_without_marker_cannot_be_claimed() {
    let text = "# Thoughts\n## AI\nI wrote this section myself.\n";
    assert!(
        Note::parse(text)
            .unwrap()
            .set_ai_section("Erase", false)
            .is_err()
    );
}

#[test]
fn sections_must_be_unambiguous_and_trailing() {
    for body in [
        format!("## AI\n{AI_MARKER}\nAI\n## Human\nKeep"),
        format!("## AI\n{AI_MARKER}\nAI\n## AI\n{AI_MARKER}\nSecond"),
    ] {
        assert!(
            Note::parse(&body)
                .unwrap()
                .set_ai_section("Erase", false)
                .is_err()
        );
    }
    for body in [
        "## Notes\nKeep\n## More\nKeep",
        "## Notes\nFirst\n## Notes\nSecond",
    ] {
        let text = format!("---\nauthor: ai\n---\n{body}");
        assert!(
            Note::parse(&text)
                .unwrap()
                .replace_ai_body("Erase")
                .is_err()
        );
    }
}

#[test]
fn ai_edits_preserve_frontmatter_and_notes_verbatim() {
    let prefix = "---\r\nauthor: ai # ownership\r\ncustom: 'value'\r\n---\r\n";
    let suffix = "## Notes\r\nHuman words.  \r\n### Detail\r\nKeep every byte.\r\n";
    let text = format!("{prefix}# Old\r\nAI words\r\n{suffix}");
    let edited = Note::parse(&text)
        .unwrap()
        .replace_ai_body("# New\nAI replacement")
        .unwrap();
    assert!(edited.starts_with(prefix));
    assert!(edited.ends_with(suffix));
}

#[test]
fn fake_headings_in_code_quotes_and_lists_are_not_sections() {
    let text = "---\nauthor: ai\n---\n# Example\n```md\n## Notes\nnot protected\n```\n> ## Notes\n> not protected\n\n- ## Notes\n\n## Notes\nHuman suffix\n";
    let edited = Note::parse(text)
        .unwrap()
        .replace_ai_body("# Replaced")
        .unwrap();
    assert!(edited.ends_with("## Notes\nHuman suffix\n"));
    assert!(!edited.contains("not protected"));
}

#[test]
fn unclosed_code_or_html_cannot_hide_protected_sections() {
    let text = new_ai_note("AI", "Existing").unwrap();
    for replacement in [
        "# Updated\n```",
        "# Updated\n~~~",
        "# Updated\n<!--",
        "# Updated\n<script>",
    ] {
        assert!(
            Note::parse(&text)
                .unwrap()
                .replace_ai_body(replacement)
                .is_err(),
            "{replacement}"
        );
        assert!(new_ai_note("AI", replacement).is_err(), "{replacement}");
    }
    assert!(
        Note::parse("Human\n```")
            .unwrap()
            .set_ai_section("comment", true)
            .is_err()
    );
}

#[test]
fn adding_up_preserves_original_yaml_and_human_body() {
    let original = "---\r\nauthor: human # keep comment\r\ncustom: 123\r\n---\r\nHuman body";
    let result = Note::parse(original).unwrap().add_up("Example").unwrap();
    assert!(result.contains("author: human # keep comment\r\ncustom: 123\r\n---\r\nHuman body"));
    assert!(result.contains("up: \"[[Example]]\""));
    assert!(Note::parse(&result).unwrap().add_up("Other").is_err());
}

#[test]
fn stale_revision_and_section_injection_are_rejected() {
    let text = "Human text";
    let note = Note::parse(text).unwrap();
    assert!(note.check_revision(&revision("Old text")).is_err());
    assert!(note.check_revision(&revision(text)).is_ok());
    for content in ["## Human\nMisleading", "# Another note", AI_MARKER] {
        assert!(note.set_ai_section(content, false).is_err());
    }
}
