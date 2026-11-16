//! Rebuildable, local inverted index. No database service or model is needed.
//! Every query hashes current files, so external edits are visible immediately.
use crate::{
    memory::{self, Memory},
    note::{self, Note},
    vault::{Vault, category},
};
use anyhow::{Result, ensure};
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

const VERSION: u32 = 2;
// Standard BM25 parameters.
const K1: f64 = 1.2;
const B: f64 = 0.75;
// Vaults mix English and French; a token is indexed under both stems.
static STEMMERS: LazyLock<[Stemmer; 2]> = LazyLock::new(|| {
    [
        Stemmer::create(Algorithm::English),
        Stemmer::create(Algorithm::French),
    ]
});

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
pub(crate) fn words(text: &str) -> BTreeSet<String> {
    tokens(text).into_iter().collect()
}
fn stems(token: &str) -> BTreeSet<String> {
    STEMMERS
        .iter()
        .map(|s| s.stem(token).into_owned())
        .collect()
}
pub(crate) fn round(score: f64) -> f64 {
    (score * 1000.0).round() / 1000.0
}
/// Stem counts and token length of one indexed field.
#[derive(Serialize, Deserialize)]
pub(crate) struct Terms {
    counts: BTreeMap<String, u32>,
    length: u32,
}
impl Terms {
    fn of(text: &str) -> Self {
        let tokens = tokens(text);
        let mut per_token = BTreeMap::<&str, u32>::new();
        for t in &tokens {
            *per_token.entry(t).or_default() += 1;
        }
        let mut counts = BTreeMap::new();
        for (token, n) in per_token {
            for stem in stems(token) {
                *counts.entry(stem).or_default() += n;
            }
        }
        Terms {
            counts,
            length: tokens.len() as u32,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Document {
    pub revision: String,
    pub title: Option<String>,
    terms: Terms,
    memory_terms: Option<Terms>,
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
    /// BM25 with OR semantics. A query word scores its best-matching stem,
    /// so the English and French variants of one word never count twice.
    pub(crate) fn scores(&self, query: &str, memory_only: bool) -> BTreeMap<String, f64> {
        let (postings, field): (_, fn(&Document) -> Option<&Terms>) = if memory_only {
            (&self.memory_postings, |d| d.memory_terms.as_ref())
        } else {
            (&self.postings, |d| Some(&d.terms))
        };
        let fields: Vec<_> = self.documents.values().filter_map(field).collect();
        let n = fields.len() as f64;
        let average = fields.iter().map(|t| t.length as f64).sum::<f64>() / n.max(1.0);
        let mut scores = BTreeMap::new();
        for word in words(query) {
            let mut best = BTreeMap::<&String, f64>::new();
            for stem in stems(&word) {
                let Some(paths) = postings.get(&stem) else {
                    continue;
                };
                let df = paths.len() as f64;
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                for path in paths {
                    let Some(t) = self.documents.get(path).and_then(field) else {
                        continue;
                    };
                    let f = t.counts.get(&stem).copied().unwrap_or_default() as f64;
                    let norm = K1 * (1.0 - B + B * t.length as f64 / average.max(1.0));
                    let score = idf * f * (K1 + 1.0) / (f + norm);
                    let entry = best.entry(path).or_default();
                    *entry = entry.max(score);
                }
            }
            for (path, score) in best {
                *scores.entry(path.clone()).or_default() += score;
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
            .filter(|i: &Index| i.version == VERSION)
            .unwrap_or_default();
        let paths = self.paths()?;
        let live: BTreeSet<_> = paths.iter().cloned().collect();
        let old_len = index.documents.len();
        index.documents.retain(|p, _| live.contains(p));
        let mut changed = rebuild || index.version != VERSION || index.documents.len() != old_len;
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
                    terms: Terms::of(&format!("{path} {text}")),
                    memory_terms: memory
                        .as_ref()
                        .map(|m| Terms::of(&format!("{} {}", m.title, m.content))),
                    memory,
                    warning,
                },
            );
        }
        if changed {
            index.version = VERSION;
            index.postings.clear();
            index.memory_postings.clear();
            for (path, doc) in &index.documents {
                for stem in doc.terms.counts.keys() {
                    index
                        .postings
                        .entry(stem.clone())
                        .or_default()
                        .insert(path.clone());
                }
                for stem in doc.memory_terms.iter().flat_map(|t| t.counts.keys()) {
                    index
                        .memory_postings
                        .entry(stem.clone())
                        .or_default()
                        .insert(path.clone());
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
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total = ranked.len();
        let notes: Vec<_>=ranked.into_iter().skip(offset).take(limit).map(|(path,score)| {
            let d=&index.documents[&path];
            json!({"path":path,"revision":d.revision,"title":d.title,"score":round(score),"parse_error":d.warning})
        }).collect();
        Ok(
            json!({"total":total,"notes":notes,"next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None},"ranking":"BM25 over English and French stems of Unicode-lowercase words, OR semantics; no semantic inference."}),
        )
    }
}
