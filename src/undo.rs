//! Local, revision-checked reversal of the last individually applied review move.
use crate::{
    note::{Author, Note},
    review::{Decision, ReviewSession, is_index},
    vault::{Change, Outcome, Vault},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct UndoPlan {
    pub id: String,
    pub created: DateTime<Utc>,
    pub plan_id: String,
    pub index: usize,
    pub applied_plan_id: String,
    pub previous_item_plan_id: Option<String>,
    pub remaining_stack: Vec<usize>,
    pub snapshot: BTreeMap<String, String>,
    pub changes: Vec<Change>,
    pub archived_indexes: Vec<String>,
}

impl Vault {
    pub(crate) fn load_review_undo(&self, id: &str) -> Result<UndoPlan> {
        uuid::Uuid::parse_str(id).context("invalid undo ID")?;
        let undo: UndoPlan = serde_json::from_str(
            &self
                .read_state(&format!("undo-{id}.json"))?
                .context("undo preview not found")?,
        )?;
        ensure!(undo.id == id, "undo ID does not match its record");
        self.load_plan(&undo.plan_id)?;
        Ok(undo)
    }

    /// Only prepares a local preview; it does not alter notes or approve Undo.
    pub fn review_undo_plan(&self, id: &str) -> Result<UndoPlan> {
        let _lock = self.lock()?;
        self.require_recovered()?;
        let original = self.load_plan(id)?;
        let session = self.review_session(id)?;
        let index = *session
            .undo_stack
            .last()
            .context("no individual move is available to undo")?;
        let applied_id = session
            .applied_items
            .get(&index)
            .context("last move has no receipt")?;
        ensure!(
            session.last_item_plan_id.as_ref() == Some(applied_id),
            "undo history does not match the last move"
        );
        ensure!(
            self.read_state(&format!("applied-{applied_id}.json"))?
                .is_some(),
            "last move has no application receipt"
        );
        ensure!(
            self.read_state(&format!("undone-{applied_id}.json"))?
                .is_none(),
            "move was already undone"
        );
        let applied = self.load_plan(applied_id)?;
        ensure!(
            applied.moves.len() == 1,
            "Undo supports individual item saves, not an older batch application"
        );
        ensure!(
            applied.moves[0].source == original.moves[index].source
                && applied.moves[0].destination == original.moves[index].destination,
            "undo receipt does not match this review item"
        );
        let expected = self.review_expected_snapshot(&original, &session)?;
        ensure!(
            self.snapshot()? == expected,
            "vault changed since the last move; Undo refused to preserve later edits"
        );
        let undo_id = uuid::Uuid::new_v4().to_string();
        let mut changes = Vec::new();
        let mut archived_indexes = Vec::new();
        for change in &applied.changes {
            changes.push(Change {
                path: change.path.clone(),
                before: change.after.clone(),
                after: change.before.clone(),
            });
            if change.before.is_none() && change.path != applied.moves[0].destination {
                let text = change
                    .after
                    .as_ref()
                    .context("created file has no recorded content")?;
                ensure!(
                    is_index(&change.path) && Note::parse(text)?.author == Author::Ai,
                    "Undo cannot remove an unrecognized created file"
                );
                let name = change
                    .path
                    .rsplit('/')
                    .next()
                    .context("index has no filename")?;
                // A distinct filename prevents an archived index from competing
                // with a newly recreated index in short wikilink resolution.
                let archive = format!("4 Archive/Review undo/{undo_id} - {name}");
                // Archived indexes must survive subsequent Undo/reapply steps.
                // Filenames are unique for these moves; short links follow the
                // note without retaining a destination that Undo just removed.
                let mut archived_text = text.clone();
                for item in &original.moves {
                    let name = item
                        .source
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .trim_end_matches(".md");
                    for path in [&item.source, &item.destination] {
                        for target in [path.as_str(), path.trim_end_matches(".md")] {
                            for suffix in ["]]", "|", "#"] {
                                archived_text = archived_text.replace(
                                    &format!("[[{target}{suffix}"),
                                    &format!("[[{name}{suffix}"),
                                );
                            }
                        }
                    }
                }
                ensure!(
                    !self.note_path(&archive)?.exists(),
                    "undo archive destination already exists"
                );
                changes.push(Change {
                    path: archive.clone(),
                    before: None,
                    after: Some(archived_text),
                });
                archived_indexes.push(archive);
            }
        }
        let mut remaining_stack = session.undo_stack.clone();
        remaining_stack.pop();
        let previous_item_plan_id = remaining_stack
            .last()
            .and_then(|i| session.applied_items.get(i))
            .cloned();
        let undo = UndoPlan {
            id: undo_id,
            created: Utc::now(),
            plan_id: id.into(),
            index,
            applied_plan_id: applied_id.clone(),
            previous_item_plan_id,
            remaining_stack,
            snapshot: expected,
            changes,
            archived_indexes,
        };
        self.write_state(
            &format!("undo-{}.json", undo.id),
            &serde_json::to_string_pretty(&undo)?,
        )?;
        Ok(undo)
    }

    pub(crate) fn finish_review_undo(
        &self,
        session: &mut ReviewSession,
        undo: &UndoPlan,
    ) -> Result<()> {
        ensure!(
            session.plan_id == undo.plan_id && undo.index < session.items.len(),
            "Undo does not belong to this review"
        );
        let item = &mut session.items[undo.index];
        item.decision = item.before_apply.take().unwrap_or(Decision::Pending);
        session.applied_items.remove(&undo.index);
        session.undo_stack = undo.remaining_stack.clone();
        session.last_item_plan_id = undo.previous_item_plan_id.clone();
        session.last_undo_id = Some(undo.id.clone());
        session.pending_undo_id = None;
        session.pending_item = None;
        session.prepared_plan_id = None;
        Ok(())
    }

    /// Local CLI only: an explicit Undo action approves this exact displayed reversal.
    pub fn undo_review_item(&self, id: &str, undo_id: &str) -> Result<Outcome> {
        let _lock = self.lock()?;
        self.require_recovered()?;
        let original = self.load_plan(id)?;
        let mut session = self.review_session(id)?;
        let undo = self.load_review_undo(undo_id)?;
        ensure!(
            undo.plan_id == id
                && session.undo_stack.last() == Some(&undo.index)
                && session.applied_items.get(&undo.index) == Some(&undo.applied_plan_id),
            "Undo preview no longer matches the last move"
        );
        ensure!(
            Utc::now() - undo.created < Duration::hours(24),
            "undo preview expired; inspect a fresh Undo preview"
        );
        ensure!(
            self.read_state(&format!("undone-{}.json", undo.applied_plan_id))?
                .is_none(),
            "move was already undone"
        );
        let expected = self.review_expected_snapshot(&original, &session)?;
        ensure!(
            undo.snapshot == expected && self.snapshot()? == expected,
            "vault changed since the Undo preview; Undo refused to preserve later edits"
        );
        session.pending_undo_id = Some(undo.id.clone());
        self.save_review(&session)?;
        let outcome = self.apply(&undo.changes, "undo inbox move")?;
        self.write_state(
            &format!("undone-{}.json", undo.applied_plan_id),
            &serde_json::json!({"undo_id": undo.id, "outcome": outcome}).to_string(),
        )?;
        self.finish_review_undo(&mut session, &undo)?;
        self.save_review(&session)?;
        Ok(outcome)
    }
}
