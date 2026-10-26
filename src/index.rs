//! Rebuildable, local inverted index. No database service or model is needed.
//! Every query hashes current files, so external edits are visible immediately.
use crate::{
    memory::{self, Memory},
    note::{self, Note},
    vault::{Vault, category},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Document {
    pub revision: String,
    pub title: Option<String>,
    pub words: BTreeSet<String>,
    pub memory: Option<Memory>,
    pub warning: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Index {
    version: u32,
    pub documents: BTreeMap<String, Document>,
    postings: BTreeMap<String, BTreeSet<String>>,
    memory_postings: BTreeMap<String, BTreeSet<String>>,
}
impl Index {
    pub(crate) fn scores(&self, query: &str, memory_only: bool) -> BTreeMap<String, usize> {
        let mut scores = BTreeMap::new();
        let postings = if memory_only {
            &self.memory_postings
        } else {
            &self.postings
        };
        for word in words(query) {
            if let Some(paths) = postings.get(&word) {
                for path in paths {
                    *scores.entry(path.clone()).or_default() += 1;
                }
            }
        }
        scores
    }
}
impl Vault {
    pub(crate) fn index(&self, rebuild: bool) -> Result<Index> {
        let cached = if rebuild {
            None
        } else {
            self.read_state("search-index.json")?
        };
        let mut index: Index = cached
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .filter(|i: &Index| i.version == 1)
            .unwrap_or_default();
        let paths = self.paths()?;
        let live: BTreeSet<_> = paths.iter().cloned().collect();
        let old_len = index.documents.len();
        index.documents.retain(|p, _| live.contains(p));
        let mut changed = rebuild || index.version != 1 || index.documents.len() != old_len;
        for path in paths {
            let text = self.read_text(&path)?;
            let revision = note::revision(&text);
            if index
                .documents
                .get(&path)
                .is_some_and(|d| d.revision == revision)
            {
                continue;
            }
            changed = true;
            let (title, memory, warning) = match Note::parse(&text) {
                Ok(n) => {
                    let memory = if memory::is_memory(&n) {
                        Some(memory::record(&path, &text))
                    } else {
                        None
                    };
                    match memory {
                        Some(Ok(m)) => (n.title(), Some(m), None),
                        Some(Err(e)) => (n.title(), None, Some(e.to_string())),
                        None => (n.title(), None, None),
                    }
                }
                Err(e) => (None, None, Some(e.to_string())),
            };
            index.documents.insert(
                path.clone(),
                Document {
                    revision,
                    title,
                    words: words(&format!("{path} {text}")),
                    memory,
                    warning,
                },
            );
        }
        if changed {
            index.version = 1;
            index.postings.clear();
            index.memory_postings.clear();
            for (path, doc) in &index.documents {
                for word in &doc.words {
                    index
                        .postings
                        .entry(word.clone())
                        .or_default()
                        .insert(path.clone());
                }
                if let Some(m) = &doc.memory {
                    for word in words(&format!("{} {}", m.title, m.content)) {
                        index
                            .memory_postings
                            .entry(word)
                            .or_default()
                            .insert(path.clone());
                    }
                }
            }
            self.write_state("search-index.json", &serde_json::to_string(&index)?)?;
        }
        // Review deadlines change even when files do not.
        for doc in index.documents.values_mut() {
            if let Some(m) = &mut doc.memory {
                m.review_due = m.review_after.as_ref().is_some_and(|d| {
                    d.as_str() <= chrono::Utc::now().format("%Y-%m-%d").to_string().as_str()
                });
            }
        }
        Ok(index)
    }
    pub fn rebuild_index(&self) -> Result<Value> {
        let _lock = self.lock()?;
        let index = self.index(true)?;
        Ok(json!({"documents":index.documents.len(),"terms":index.postings.len(),"rebuilt":true}))
    }
    pub fn search_notes(
        &self,
        query: &str,
        area: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        ensure!((1..=100).contains(&limit), "limit must be 1–100");
        ensure!(!words(query).is_empty(), "query needs at least one word");
        if let Some(a) = area {
            ensure!(
                crate::vault::PARA.iter().any(|(k, _, _)| *k == a),
                "unknown PARA category"
            );
        }
        let index = self.index(false)?;
        let mut ranked: Vec<_> = index
            .scores(query, false)
            .into_iter()
            .filter(|(p, _)| area.is_none_or(|a| category(p) == Some(a)))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total = ranked.len();
        let notes: Vec<_>=ranked.into_iter().skip(offset).take(limit).map(|(path,score)| {
            let d=&index.documents[&path];
            json!({"path":path,"revision":d.revision,"title":d.title,"score":score,"parse_error":d.warning})
        }).collect();
        Ok(
            json!({"total":total,"notes":notes,"next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None},"ranking":"Distinct query words matched, OR semantics, Unicode lowercase; no stemming or semantic inference."}),
        )
    }
}
