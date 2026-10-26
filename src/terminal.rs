use anyhow::{Context, Result, ensure};
use mcp_brain::{
    review::{Decision, Plan, ReviewSession},
    vault::{Change, Vault},
};
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, Write};

#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

pub fn plan_listing(
    out: &mut impl Write,
    listing: &mcp_brain::plans::PlanListing,
    json: bool,
    style: Style,
) -> Result<()> {
    if json {
        return value(out, listing, true, style);
    }
    writeln!(out, "{}", style.heading("Recent Inbox plans"))?;
    writeln!(
        out,
        "{:<36}  {:<9}  {:<9}  Notes",
        "Plan", "Status", "Moved"
    )?;
    for plan in &listing.plans {
        writeln!(
            out,
            "{:<36}  {:<9}  {:<9}  {}",
            plan.id,
            plan.status,
            format!("{}/{}", plan.applied, plan.moves),
            cell(&plan.names.join(" · "), 40)
        )?;
    }
    if listing.plans.is_empty() {
        writeln!(
            out,
            "No saved proposals. Ask the assistant to prepare an Inbox plan."
        )?;
    }
    for warning in &listing.warnings {
        writeln!(out, "{}", style.paint(warning, "33"))?;
    }
    writeln!(
        out,
        "\nUse inbox review to choose a plan, or inbox review PLAN_ID --plain for a basic terminal."
    )?;
    Ok(())
}

impl Style {
    fn paint(self, text: &str, code: &str) -> String {
        let text = safe(text);
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text
        }
    }

    fn heading(self, text: &str) -> String {
        self.paint(text, "1;36")
    }
}

// Note text is untrusted terminal input; do not execute embedded control sequences.
pub(super) fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn cell(text: &str, width: usize) -> String {
    let text = safe(text).replace(['\n', '\t'], " ");
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() {
        format!(
            "{}…",
            prefix
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        prefix
    }
}

fn cell_limit(columns: usize) -> usize {
    (80 / columns.max(1)).saturating_sub(2).clamp(12, 42)
}

fn table(out: &mut impl Write, headers: &[&str], rows: &[Vec<String>], style: Style) -> Result<()> {
    let limit = cell_limit(headers.len());
    let widths: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            rows.iter()
                .map(|r| cell(&r[i], limit).chars().count())
                .max()
                .unwrap_or(0)
                .max(h.len())
        })
        .collect();
    for (i, header) in headers.iter().enumerate() {
        write!(out, "{:<width$}  ", header, width = widths[i])?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        style.paint(
            &"─".repeat(widths.iter().sum::<usize>() + 2 * headers.len()),
            "2"
        )
    )?;
    for row in rows {
        for (i, value) in row.iter().enumerate() {
            write!(out, "{:<width$}  ", cell(value, limit), width = widths[i])?;
        }
        writeln!(out)?;
    }
    Ok(())
}

pub fn value(out: &mut impl Write, value: &impl Serialize, json: bool, style: Style) -> Result<()> {
    let value = serde_json::to_value(value)?;
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
    } else {
        human_value(out, &value, style, 0)?;
    }
    Ok(())
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "none".into(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        other => other.to_string(),
    }
}

fn human_value(out: &mut impl Write, value: &Value, style: Style, depth: usize) -> Result<()> {
    match value {
        Value::Object(fields) => {
            for (key, v) in fields {
                if key == "revisions" && fields.contains_key("changed") {
                    continue;
                }
                let label = key.replace('_', " ");
                if v.is_object() || v.is_array() {
                    writeln!(out, "\n{}", style.heading(&label))?;
                    human_value(out, v, style, depth + 1)?;
                } else {
                    writeln!(out, "{}: {}", style.paint(&label, "1"), safe(&scalar(v)))?;
                }
            }
        }
        Value::Array(rows) if rows.is_empty() => writeln!(out, "  none")?,
        Value::Array(rows) if rows.iter().all(Value::is_object) => {
            let candidates = [
                "title",
                "path",
                "source",
                "destination",
                "decision",
                "comment",
                "scope",
                "folder",
                "count",
                "reason",
                "rule",
                "memory_type",
            ];
            let keys: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|key| rows.iter().any(|r| !r[*key].is_null()))
                .take(5)
                .collect();
            if keys.is_empty() {
                for (i, row) in rows.iter().enumerate() {
                    writeln!(out, "\n{}", style.heading(&format!("Item {}", i + 1)))?;
                    human_value(out, row, style, depth + 1)?;
                }
            } else {
                let data: Vec<_> = rows
                    .iter()
                    .map(|r| keys.iter().map(|k| scalar(&r[*k])).collect())
                    .collect();
                table(out, &keys, &data, style)?;
                for (i, row) in rows.iter().enumerate() {
                    let remaining: serde_json::Map<_, _> = row
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(key, v)| {
                            !keys.contains(&key.as_str())
                                || scalar(v).chars().count() > cell_limit(keys.len())
                        })
                        .map(|(key, v)| (key.clone(), v.clone()))
                        .collect();
                    if !remaining.is_empty() {
                        writeln!(
                            out,
                            "\n{}",
                            style.heading(&format!("Item {} details", i + 1))
                        )?;
                        human_value(out, &Value::Object(remaining), style, depth + 1)?;
                    }
                }
            }
        }
        Value::Array(rows) => {
            for row in rows {
                if row.is_object() || row.is_array() {
                    human_value(out, row, style, depth + 1)?;
                } else {
                    writeln!(out, "{}• {}", "  ".repeat(depth.min(4)), safe(&scalar(row)))?;
                }
            }
        }
        other => writeln!(out, "{}", safe(&scalar(other)))?,
    }
    Ok(())
}

pub fn plan(
    out: &mut impl Write,
    plan: &Plan,
    session: Option<&ReviewSession>,
    details: bool,
    style: Style,
) -> Result<()> {
    writeln!(out, "{}", style.heading("Inbox move proposal"))?;
    writeln!(out, "Plan: {}", safe(&plan.id))?;
    writeln!(
        out,
        "{} moves · {} affected files · expires {}",
        plan.moves.len(),
        plan.changes.len(),
        plan.created + chrono::Duration::hours(24)
    )?;
    let rows: Vec<_> = plan
        .moves
        .iter()
        .enumerate()
        .map(|(i, item)| {
            vec![
                (i + 1).to_string(),
                item.source
                    .strip_prefix("0 Inbox/")
                    .unwrap_or(&item.source)
                    .into(),
                item.destination
                    .rsplit_once('/')
                    .map(|(d, _)| d)
                    .unwrap_or(&item.destination)
                    .into(),
                session
                    .map(|s| decision_name(s.items[i].decision))
                    .unwrap_or("selected")
                    .into(),
            ]
        })
        .collect();
    table(out, &["#", "Note", "Destination", "Decision"], &rows, style)?;
    for (i, item) in plan.moves.iter().enumerate() {
        writeln!(out, "\n{}. {}", i + 1, style.paint(&item.source, "1"))?;
        writeln!(
            out,
            "   → {}\n   {}",
            safe(&item.destination),
            safe(&item.reason)
        )?;
        if let Some(comment) = session.and_then(|s| s.items[i].comment.as_ref()) {
            writeln!(out, "   Comment: {}", safe(comment))?;
        }
    }
    if details {
        changes(out, &plan.changes, style)?;
    } else {
        writeln!(
            out,
            "\nUse inbox review {} to decide one item at a time.",
            plan.id
        )?;
        writeln!(
            out,
            "Use inbox show {} --details for every before/after value; --json for machine output.",
            plan.id
        )?;
    }
    Ok(())
}

fn decision_name(decision: Decision) -> &'static str {
    match decision {
        Decision::Pending => "pending",
        Decision::Accepted => "accepted",
        Decision::Dropped => "dropped",
        Decision::Commented => "needs precision",
        Decision::Applied => "applied",
    }
}

pub fn changes(out: &mut impl Write, changes: &[Change], style: Style) -> Result<()> {
    for change in changes {
        writeln!(out, "\n{}", style.heading(&change.path))?;
        if let Some(before) = &change.before {
            writeln!(out, "{}", style.paint("Before", "1;31"))?;
            for line in before.split('\n') {
                writeln!(out, "{}", style.paint(&format!("- {line}"), "31"))?;
            }
        }
        if let Some(after) = &change.after {
            writeln!(out, "{}", style.paint("After", "1;32"))?;
            for line in after.split('\n') {
                writeln!(out, "{}", style.paint(&format!("+ {line}"), "32"))?;
            }
        }
    }
    Ok(())
}

fn selection(out: &mut impl Write, plan: &Plan, style: Style) -> Result<()> {
    writeln!(out, "\n{}", style.heading("Apply accepted moves"))?;
    writeln!(
        out,
        "{} accepted · {} affected files",
        plan.moves.len(),
        plan.changes.len()
    )?;
    let rows: Vec<_> = plan
        .moves
        .iter()
        .map(|item| vec![item.source.clone(), item.destination.clone()])
        .collect();
    table(out, &["From", "To"], &rows, style)?;
    writeln!(out, "\n{}", style.heading("Other affected files"))?;
    for change in &plan.changes {
        if plan
            .moves
            .iter()
            .any(|m| m.source == change.path || m.destination == change.path)
        {
            continue;
        }
        let action = match (&change.before, &change.after) {
            (None, Some(_)) => "create",
            (Some(_), None) => "remove",
            _ => "update",
        };
        writeln!(out, "  {action}: {}", safe(&change.path))?;
    }
    writeln!(
        out,
        "\nOnly accepted moves are included. Index, parent metadata and link changes have been recomputed."
    )?;
    Ok(())
}

fn answer(input: &mut impl BufRead, out: &mut impl Write, prompt: &str) -> Result<Option<String>> {
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut text = String::new();
    if input.read_line(&mut text)? == 0 {
        return Ok(None);
    }
    Ok(Some(text.trim().to_owned()))
}

pub fn review(
    vault: &Vault,
    id: &str,
    input: &mut impl BufRead,
    out: &mut impl Write,
    style: Style,
) -> Result<()> {
    let proposal = vault.load_plan(id)?;
    let mut session = vault.review_session(id)?;
    ensure!(
        !vault.review_applied(&session)?,
        "review was already applied"
    );
    let mut index = session
        .items
        .iter()
        .position(|r| r.decision == Decision::Pending)
        .unwrap_or(0);
    loop {
        let item = proposal.moves.get(index).context("plan has no items")?;
        let accepted = session
            .items
            .iter()
            .filter(|r| r.decision == Decision::Accepted)
            .count();
        writeln!(
            out,
            "\n{}",
            style.heading(&format!(
                "Item {} of {} · {} accepted",
                index + 1,
                proposal.moves.len(),
                accepted
            ))
        )?;
        writeln!(
            out,
            "{}\n→ {}\n\n{}",
            style.paint(&item.source, "1"),
            safe(&item.destination),
            safe(&item.reason)
        )?;
        writeln!(
            out,
            "Decision: {}",
            decision_name(session.items[index].decision)
        )?;
        if let Some(comment) = &session.items[index].comment {
            writeln!(out, "Comment: {}", safe(comment))?;
        }
        if let Some(change) = proposal.changes.iter().find(|c| c.path == item.source)
            && let Some(text) = &change.before
        {
            writeln!(out, "\n{}\n{}", style.heading("Original note"), safe(text))?;
        }
        writeln!(
            out,
            "\n[a] Accept move  [d] Drop proposal  [c] Comment / request precision"
        )?;
        writeln!(
            out,
            "[n] Next  [p] Previous  [v] Item changes  [f] Finish  [q] Save and quit"
        )?;
        let Some(action) = answer(input, out, "> ")? else {
            writeln!(out, "\nReview saved; no additional notes moved.")?;
            return Ok(());
        };
        match action.to_lowercase().as_str() {
            "a" | "d" => {
                let decision = if action.eq_ignore_ascii_case("a") {
                    Decision::Accepted
                } else {
                    Decision::Dropped
                };
                session = vault.record_review(id, index, decision, None)?;
                index = (index + 1).min(proposal.moves.len() - 1);
            }
            "c" => {
                if let Some(comment) =
                    answer(input, out, "Precision for the assistant (empty cancels): ")?
                    && !comment.is_empty()
                {
                    session = vault.record_review(id, index, Decision::Commented, Some(comment))?;
                    index = (index + 1).min(proposal.moves.len() - 1);
                }
            }
            "n" => index = (index + 1) % proposal.moves.len(),
            "p" => {
                index = if index == 0 {
                    proposal.moves.len() - 1
                } else {
                    index - 1
                }
            }
            "v" => {
                let selected: Vec<_> = proposal
                    .changes
                    .iter()
                    .filter(|c| c.path == item.source || c.path == item.destination)
                    .cloned()
                    .collect();
                changes(out, &selected, style)?;
            }
            "f" => {
                if accepted == 0 {
                    writeln!(
                        out,
                        "No accepted moves. Feedback is saved; use q to leave notes in Inbox."
                    )?;
                    continue;
                }
                let selected = vault.prepare_review_plan(id)?;
                selection(out, &selected, style)?;
                let prompt = if selected.moves.len() == 1 {
                    "\nMove this note? Type apply, v for exact changes, or Enter to go back: "
                        .into()
                } else {
                    format!(
                        "\nMove these {} notes? Type apply, v for exact changes, or Enter to go back: ",
                        selected.moves.len()
                    )
                };
                let confirmation = loop {
                    let response = answer(input, out, &prompt)?;
                    if response.as_deref() == Some("v") {
                        changes(out, &selected.changes, style)?;
                    } else {
                        break response;
                    }
                };
                if confirmation.as_deref() == Some("apply") {
                    let outcome = vault.apply_plan(&selected.id)?;
                    writeln!(
                        out,
                        "\nMoved {} accepted {}. Other notes remain in Inbox.",
                        selected.moves.len(),
                        if selected.moves.len() == 1 {
                            "note"
                        } else {
                            "notes"
                        }
                    )?;
                    if let Some(warning) = outcome.warning {
                        writeln!(out, "{}", style.paint(&warning, "33"))?;
                    }
                    return Ok(());
                }
                writeln!(out, "No notes changed. Decisions and comments are saved.")?;
            }
            "q" => {
                writeln!(
                    out,
                    "Review saved. No notes moved. Resume with inbox review {id}."
                )?;
                writeln!(
                    out,
                    "The assistant can read comments with inbox_plan_review; CLI: inbox feedback {id} --json."
                )?;
                return Ok(());
            }
            _ => writeln!(out, "Choose a, d, c, n, p, v, f or q.")?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_output_neutralizes_note_control_sequences() {
        let mut output = Vec::new();
        value(
            &mut output,
            &serde_json::json!({"title": "note\u{1b}[2J\rspoof"}),
            false,
            Style { color: false },
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains(['\u{1b}', '\r']));
        assert!(text.contains("spoof"));
    }

    fn fixture() -> (tempfile::TempDir, Vault, String) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        std::fs::create_dir_all(root.join("0 Inbox")).unwrap();
        let vault = Vault::open(&root, Some(temp.path().join("state"))).unwrap();
        let moves = ["A", "B", "C"].map(|name| {
            let source = format!("0 Inbox/{name}.md");
            let text = format!("# {name}\nHuman text stays exact.\n");
            std::fs::write(root.join(&source), &text).unwrap();
            mcp_brain::review::Move {
                source,
                destination: format!("3 Resources/{name}.md"),
                expected_revision: mcp_brain::note::revision(&text),
                reason: "Reference without an obligation".into(),
            }
        });
        let proposal = vault.propose_plan(moves.into()).unwrap();
        (temp, vault, proposal["id"].as_str().unwrap().into())
    }

    #[test]
    fn interactive_review_applies_only_accepted_items_and_keeps_precision() {
        let (_temp, vault, id) = fixture();
        let mut input = std::io::Cursor::new(
            b"a\nd\nc\nThis belongs to an ongoing responsibility.\nf\nv\napply\n",
        );
        let mut output = Vec::new();
        review(&vault, &id, &mut input, &mut output, Style { color: false }).unwrap();
        assert!(
            vault
                .read_text("3 Resources/A.md")
                .unwrap()
                .ends_with("# A\nHuman text stays exact.\n")
        );
        assert!(
            vault
                .read_text("0 Inbox/B.md")
                .unwrap()
                .ends_with("# B\nHuman text stays exact.\n")
        );
        assert!(
            vault
                .read_text("0 Inbox/C.md")
                .unwrap()
                .ends_with("# C\nHuman text stays exact.\n")
        );
        assert!(!vault.root.join("3 Resources/B.md").exists());
        assert!(!vault.root.join("3 Resources/C.md").exists());
        let feedback = vault.inbox_plan_review(&id).unwrap();
        assert_eq!(feedback["items"][1]["decision"], "dropped");
        assert_eq!(
            feedback["items"][2]["comment"],
            "This belongs to an ongoing responsibility."
        );
        assert_eq!(feedback["applied"], true);
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Move this note?")
        );
    }

    #[test]
    fn eof_or_cancelled_confirmation_never_applies_accepted_moves() {
        let (_temp, vault, id) = fixture();
        let before = vault.snapshot().unwrap();
        let mut input = std::io::Cursor::new(b"a\nf\ncancel\nq\n");
        review(
            &vault,
            &id,
            &mut input,
            &mut Vec::new(),
            Style { color: false },
        )
        .unwrap();
        assert_eq!(vault.snapshot().unwrap(), before);
        let mut eof = std::io::Cursor::new(b"");
        review(
            &vault,
            &id,
            &mut eof,
            &mut Vec::new(),
            Style { color: false },
        )
        .unwrap();
        assert_eq!(vault.snapshot().unwrap(), before);
        assert_eq!(
            vault.inbox_plan_review(&id).unwrap()["items"][0]["decision"],
            "accepted"
        );
    }
}
