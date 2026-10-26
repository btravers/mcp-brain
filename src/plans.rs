//! Human-facing saved proposals, excluding internal item/subset previews.
use crate::vault::Vault;
use anyhow::{Result, ensure};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::{collections::BTreeSet, fs};

#[derive(Debug, Serialize)]
pub struct PlanSummary {
    pub id: String,
    pub created: DateTime<Utc>,
    pub expires: DateTime<Utc>,
    pub moves: usize,
    pub applied: usize,
    pub commented: usize,
    pub dropped: usize,
    pub status: String,
    pub names: Vec<String>,
    pub reviewed: bool,
}

#[derive(Debug, Serialize)]
pub struct PlanListing {
    pub plans: Vec<PlanSummary>,
    pub warnings: Vec<String>,
}

impl Vault {
    pub fn inbox_plans(&self, limit: usize) -> Result<PlanListing> {
        ensure!((1..=200).contains(&limit), "plan limit must be 1–200");
        let snapshot = self.snapshot()?;
        let mut plans = Vec::new();
        let mut warnings = Vec::new();
        let mut referenced = BTreeSet::new();
        for entry in fs::read_dir(&self.state)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("plan-")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_err() {
                continue;
            }
            let plan = match self.load_plan(id) {
                Ok(plan) => plan,
                Err(error) => {
                    warnings.push(format!("Could not read saved plan {id}: {error}"));
                    continue;
                }
            };
            if plan.parent_plan_id.is_some() {
                continue;
            }
            let session = match self.review_session(id) {
                Ok(session) => session,
                Err(error) => {
                    warnings.push(format!("Could not read review {id}: {error}"));
                    continue;
                }
            };
            if let Some(id) = &session.prepared_plan_id {
                referenced.insert(id.clone());
            }
            for id in session.applied_items.values() {
                if id != &session.plan_id {
                    referenced.insert(id.clone());
                }
            }
            let applied = session.applied_items.len();
            let commented = session
                .items
                .iter()
                .filter(|r| r.decision == crate::review::Decision::Commented)
                .count();
            let dropped = session
                .items
                .iter()
                .filter(|r| r.decision == crate::review::Decision::Dropped)
                .count();
            let reviewed = self.has_review(id)?;
            let fresh = self
                .review_expected_snapshot(&plan, &session)
                .is_ok_and(|expected| expected == snapshot);
            let expires = plan.created + Duration::hours(24);
            let status = if applied == plan.moves.len() {
                "Complete"
            } else if expires <= Utc::now() {
                "Expired"
            } else if !fresh {
                "Stale"
            } else {
                "Ready"
            };
            let names = plan
                .moves
                .iter()
                .take(3)
                .map(|m| {
                    m.source
                        .rsplit('/')
                        .next()
                        .unwrap_or(&m.source)
                        .trim_end_matches(".md")
                        .to_owned()
                })
                .collect();
            plans.push(PlanSummary {
                id: plan.id,
                created: plan.created,
                expires,
                moves: plan.moves.len(),
                applied,
                commented,
                dropped,
                status: status.into(),
                names,
                reviewed,
            });
        }
        // Older reviews did not tag their internal previews; hide referenced receipts too.
        plans.retain(|plan| !referenced.contains(&plan.id));
        plans.sort_by(|a, b| {
            let rank = |p: &PlanSummary| match (p.status.as_str(), p.reviewed) {
                ("Ready", true) => 0,
                ("Ready", false) => 1,
                ("Stale", _) => 2,
                ("Expired", _) => 3,
                _ => 4,
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| b.created.cmp(&a.created))
                .then_with(|| a.id.cmp(&b.id))
        });
        plans.truncate(limit);
        Ok(PlanListing { plans, warnings })
    }
}
