use crate::{
    note::{self, Author, Note},
    vault::{Change, Outcome, PARA, Vault, category, valid_relative},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Move {
    /// Exact current vault-relative Inbox note path.
    pub source: String,
    /// Destination note path in projects, areas, resources, or archive. Keep the filename unchanged.
    pub destination: String,
    /// Revision returned by read_note.
    pub expected_revision: String,
    /// Explain the actionable purpose, responsibility, reference value, or reason for archiving.
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub created: DateTime<Utc>,
    pub vault_id: String,
    pub moves: Vec<Move>,
    pub snapshot: BTreeMap<String, String>,
    pub changes: Vec<Change>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub every_days: Option<u32>,
    pub last_reviewed: Option<DateTime<Utc>>,
    pub configured_at: Option<DateTime<Utc>>,
}

impl Vault {
    fn reminder(&self) -> Result<Reminder> {
        self.read_state("reminder.json")?
            .map(|s| serde_json::from_str(&s).context("invalid reminder state"))
            .unwrap_or_else(|| Ok(Reminder::default()))
    }

    pub fn configure_reminder(&self, days: Option<u32>) -> Result<Value> {
        let _lock = self.lock()?;
        if let Some(days) = days {
            ensure!(
                (1..=365).contains(&days),
                "reminder interval must be 1–365 days"
            );
        }
        let mut reminder = self.reminder()?;
        reminder.every_days = days;
        reminder.configured_at = Some(Utc::now());
        self.write_state("reminder.json", &serde_json::to_string_pretty(&reminder)?)?;
        self.inbox_status()
    }

    pub fn mark_reviewed(&self) -> Result<Value> {
        let _lock = self.lock()?;
        self.mark_reviewed_unlocked()?;
        self.inbox_status()
    }

    fn mark_reviewed_unlocked(&self) -> Result<()> {
        let mut reminder = self.reminder()?;
        reminder.last_reviewed = Some(Utc::now());
        self.write_state("reminder.json", &serde_json::to_string_pretty(&reminder)?)
    }

    fn inbox_paths(&self) -> Result<Vec<String>> {
        Ok(self
            .paths()?
            .into_iter()
            .filter(|p| category(p) == Some("inbox") && !is_index(p))
            .collect())
    }

    pub fn inbox_status(&self) -> Result<Value> {
        let paths = self.inbox_paths()?;
        let mut oldest: Option<DateTime<Utc>> = None;
        for path in &paths {
            let modified: DateTime<Utc> = self.note_path(path)?.metadata()?.modified()?.into();
            oldest = Some(oldest.map_or(modified, |old| old.min(modified)));
        }
        let reminder = self.reminder()?;
        let next = reminder.every_days.and_then(|d| {
            reminder
                .last_reviewed
                .or(reminder.configured_at)
                .map(|last| last + Duration::days(d.into()))
        });
        Ok(
            json!({"count":paths.len(),"oldest_modified":oldest,"reminder":reminder,
            "next_review":next,"due":!paths.is_empty() && next.is_some_and(|t|t<=Utc::now()),
            "suggest_configuring_reminder":!paths.is_empty() && reminder.configured_at.is_none(),
            "reminder_setup":"Ask the user whether they want a reminder. They can enable it with: mcp-brain --vault <path> reminder configure --every-days 7. The MCP cannot enable it itself.",
            "delivery":"Due status is shown when the client calls inbox_status or vault_context. Use the CLI check command with an OS scheduler for reminders outside sessions."}),
        )
    }

    pub fn inbox_review(&self, offset: usize, limit: usize) -> Result<Value> {
        ensure!((1..=50).contains(&limit), "limit must be 1–50");
        let paths = self.paths()?;
        let inbox = self.inbox_paths()?;
        let folders: BTreeSet<_> = paths
            .iter()
            .filter(|p| matches!(category(p), Some("projects" | "areas" | "resources")))
            .filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_owned()))
            .collect();
        let mut items = Vec::new();
        for path in inbox.iter().skip(offset).take(limit) {
            let text = self.read_text(path)?;
            let words = tokens(&format!("{path} {text}"));
            let mut candidates: Vec<_> = folders
                .iter()
                .map(|folder| {
                    let score = tokens(folder).intersection(&words).count();
                    (score, folder)
                })
                .filter(|(score, _)| *score > 0)
                .collect();
            candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            items.push(json!({"path":path,"revision":note::revision(&text),"preview":text.chars().take(1200).collect::<String>(),
                "related_folders":candidates.into_iter().take(5).map(|(score,folder)|json!({"folder":folder,"shared_words":score})).collect::<Vec<_>>(),
                "questions":["Does this support a goal with an end date?","Is this an ongoing responsibility?","Is this reference material, or is it no longer relevant?"],
                "guidance":"Folder matches are lexical hints, not classification decisions. Read the complete note, propose destination + reason, and ask once for the batch. Ask when two categories fit."}));
        }
        Ok(
            json!({"total":inbox.len(),"items":items,"next_offset":if offset.saturating_add(limit)<inbox.len(){Some(offset+limit)}else{None},"status":self.inbox_status()?}),
        )
    }

    pub fn propose_plan(&self, moves: Vec<Move>) -> Result<Value> {
        let _lock = self.lock()?;
        ensure!(
            !moves.is_empty() && moves.len() <= 100,
            "plan must contain 1–100 moves"
        );
        let snapshot = self.snapshot()?;
        let mut seen = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        let mut changes = Vec::new();
        let mut folders = Vec::new();
        for item in &moves {
            ensure!(
                snapshot.contains_key(&item.source),
                "source must exactly match a path returned by list_notes"
            );
            ensure!(
                category(&item.source) == Some("inbox") && !is_index(&item.source),
                "source must be an Inbox note, not its index"
            );
            ensure!(
                matches!(
                    category(&item.destination),
                    Some("projects" | "areas" | "resources" | "archive")
                ),
                "destination must be in Projects, Areas, Resources, or Archive"
            );
            valid_relative(&item.destination)?;
            ensure!(
                item.source.rsplit('/').next() == item.destination.rsplit('/').next(),
                "keep the filename unchanged to preserve short wikilinks"
            );
            ensure!(
                !item.reason.trim().is_empty(),
                "each move needs an actionability-based reason"
            );
            ensure!(
                seen.insert(&item.source) && destinations.insert(item.destination.to_lowercase()),
                "duplicate source or destination"
            );
            ensure!(
                !self.note_path(&item.destination)?.exists()
                    && !snapshot
                        .keys()
                        .any(|p| p.eq_ignore_ascii_case(&item.destination)),
                "destination already exists"
            );
            let filename = item.source.rsplit('/').next().unwrap();
            ensure!(
                snapshot
                    .keys()
                    .filter(|p| p
                        .rsplit('/')
                        .next()
                        .is_some_and(|f| f.eq_ignore_ascii_case(filename)))
                    .count()
                    == 1,
                "duplicate filenames make short wikilinks ambiguous; resolve them before moving"
            );
            let text = self.read_text(&item.source)?;
            for target in local_markdown_targets(&text) {
                ensure!(
                    target.starts_with('#'),
                    "a moved note contains a relative Markdown link or image; convert it to an unambiguous wikilink before moving"
                );
            }
            ensure!(
                !text.contains("[[./") && !text.contains("[[../"),
                "a moved note contains relative wikilinks; convert them to vault-relative links first"
            );
            let parsed = Note::parse(&text)?;
            parsed.check_revision(&item.expected_revision)?;
            let parent = item.destination.rsplit_once('/').unwrap().0;
            ensure!(
                !is_index(&item.destination),
                "destination would turn the note into an index; choose another folder"
            );
            let updated = parsed.add_up(parent.rsplit('/').next().unwrap())?;
            changes.push(Change {
                path: item.source.clone(),
                before: Some(text),
                after: None,
            });
            changes.push(Change {
                path: item.destination.clone(),
                before: None,
                after: Some(updated),
            });
            folders.push(parent.to_owned());
            folders.push(item.source.rsplit_once('/').unwrap().0.to_owned());
        }
        // Repair path-qualified links only inside AI-owned text. Human prose,
        // including protected Notes, is never silently modified for link repair.
        for path in snapshot.keys() {
            if seen.contains(path) {
                continue;
            }
            let before = self.read_text(path)?;
            let after = repair_owned_links(&before, &moves)?;
            if after != before {
                changes.push(Change {
                    path: path.clone(),
                    before: Some(before),
                    after: Some(after),
                });
            }
        }
        for c in changes.iter_mut().filter(|c| c.before.is_none()) {
            if let Some(after) = &c.after {
                c.after = Some(repair_owned_links(after, &moves)?);
            }
        }
        self.maintain_indexes(&mut changes, &folders)?;
        let plan = Plan {
            id: uuid::Uuid::new_v4().to_string(),
            created: Utc::now(),
            vault_id: note::revision(&self.root.to_string_lossy()),
            moves,
            snapshot,
            changes,
        };
        self.write_state(
            &format!("plan-{}.json", plan.id),
            &serde_json::to_string_pretty(&plan)?,
        )?;
        Ok(
            json!({"id":plan.id,"moves":plan.moves,"changed_paths":plan.changes.iter().map(|c|&c.path).collect::<Vec<_>>(),
            "expires":plan.created+Duration::hours(24),"approval":"Review the plan with inbox show <id>, then run inbox apply <id> locally. There is no MCP apply tool.",
            "command":format!("mcp-brain --vault <path> inbox apply {}",plan.id)}),
        )
    }

    pub fn load_plan(&self, id: &str) -> Result<Plan> {
        uuid::Uuid::parse_str(id).context("invalid plan ID")?;
        let plan: Plan = serde_json::from_str(
            &self
                .read_state(&format!("plan-{id}.json"))?
                .context("plan not found")?,
        )?;
        ensure!(
            plan.id == id && plan.vault_id == note::revision(&self.root.to_string_lossy()),
            "plan belongs to a different vault"
        );
        Ok(plan)
    }

    /// Called only by the local CLI after showing the plan and reading approval.
    pub fn apply_plan(&self, id: &str) -> Result<Outcome> {
        let _lock = self.lock()?;
        ensure!(
            self.read_state(&format!("applied-{id}.json"))?.is_none(),
            "plan was already applied"
        );
        let plan = self.load_plan(id)?;
        ensure!(
            Utc::now() - plan.created < Duration::hours(24),
            "plan expired; generate a fresh proposal"
        );
        ensure!(
            self.snapshot()? == plan.snapshot,
            "vault changed since the proposal; generate a new plan"
        );
        let outcome = self.apply(&plan.changes, "consolidate inbox")?;
        self.write_state(
            &format!("applied-{id}.json"),
            &serde_json::to_string(&outcome)?,
        )?;
        self.mark_reviewed_unlocked()?;
        Ok(outcome)
    }
}

fn is_index(path: &str) -> bool {
    path.rsplit_once('/').is_some_and(|(folder, file)| {
        folder
            .rsplit('/')
            .next()
            .is_some_and(|name| file == format!("{name}.md"))
    })
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_lowercase)
        .collect()
}

fn repair_owned_links(text: &str, moves: &[Move]) -> Result<String> {
    fn repair(text: &str, moves: &[Move]) -> String {
        let mut out = text.to_owned();
        for item in moves {
            let from = item.source.strip_suffix(".md").unwrap_or(&item.source);
            let to = item
                .destination
                .strip_suffix(".md")
                .unwrap_or(&item.destination);
            for suffix in ["]]", "|", "#"] {
                out = out.replace(&format!("[[{from}{suffix}"), &format!("[[{to}{suffix}"));
                out = out.replace(
                    &format!("[[{}{suffix}", item.source),
                    &format!("[[{}{suffix}", item.destination),
                );
            }
        }
        out
    }
    let expected = repair(text, moves);
    // Markdown relative paths need a proper Obsidian resolver. Refuse known
    // references instead of pretending a substring replacement is sufficient.
    for item in moves {
        let filename = item.source.rsplit('/').next().unwrap();
        let stem = filename.strip_suffix(".md").unwrap();
        for target in local_markdown_targets(text) {
            let decoded = target.replace("%20", " ");
            let file = decoded
                .split('#')
                .next()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("");
            ensure!(
                file != filename && file != stem,
                "a Markdown link to the moved note needs human repair first"
            );
        }
        for wiki in text
            .split("[[")
            .skip(1)
            .filter_map(|s| s.split_once("]]").map(|(target, _)| target))
        {
            let target = wiki.split(['#', '|']).next().unwrap_or("");
            let target_stem = target.strip_suffix(".md").unwrap_or(target);
            if target_stem.contains('/') && target_stem.rsplit('/').next() == Some(stem) {
                ensure!(
                    target_stem == item.source.strip_suffix(".md").unwrap(),
                    "a noncanonical path-qualified wikilink needs human repair before moving"
                );
            }
        }
        let encoded = item.source.replace(' ', "%20");
        ensure!(
            !text.contains(&format!("]({}", item.source))
                && !text.contains(&format!("]({encoded}")),
            "a Markdown path link needs human repair before this move"
        );
    }
    if expected == text {
        return Ok(text.into());
    }
    let parsed = Note::parse(text)?;
    if parsed.author == Author::Ai {
        let end = parsed.protected_notes_start()?.unwrap_or(text.len());
        let prefix = &text[..parsed.body_start];
        let suffix = &text[end..];
        ensure!(
            repair(prefix, moves) == prefix && repair(suffix, moves) == suffix,
            "a protected frontmatter or Notes link needs human repair before this move"
        );
        Ok(format!(
            "{prefix}{}{suffix}",
            repair(&text[parsed.body_start..end], moves)
        ))
    } else {
        let section = parsed
            .ai_section_content()?
            .context("a path-qualified link in human text needs human repair before this move")?;
        let new_section = repair(section, moves);
        let out = parsed.set_ai_section(&new_section, false)?;
        ensure!(
            repair(&out, moves) == out,
            "a path-qualified link in human text needs human repair before this move"
        );
        Ok(out)
    }
}

fn local_markdown_targets(text: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag};
    Parser::new(text)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                if !dest_url.contains(':') && !dest_url.starts_with("//") =>
            {
                Some(dest_url.to_string())
            }
            _ => None,
        })
        .collect()
}

pub fn para_reference() -> Value {
    json!(
        PARA.iter()
            .map(|(key, folder, rule)| json!({"category":key,"folder":folder,"rule":rule}))
            .collect::<Vec<_>>()
    )
}
