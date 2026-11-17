//! Optional local semantic search with multilingual-e5-small (384 dimensions,
//! about 100 languages including English and French), run on CPU with candle.
//! The model is installed explicitly into the state directory; nothing is
//! downloaded at query time. Vectors are a rebuildable cache keyed by revision.
use crate::{index::Index, note, note::Note, vault::Vault};
use anyhow::{Context, Result, ensure};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use pulldown_cmark::{Event, Parser, Tag};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const MODEL: &str = "intfloat/multilingual-e5-small";
const REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
const FILES: [(&str, &str); 3] = [
    (
        "config.json",
        "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
    ),
    (
        "tokenizer.json",
        "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    ),
    (
        "model.safetensors",
        "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
    ),
];
const STORE: &str = "embeddings.bin";
/// Sections longer than this split at line breaks; the model reads 512 tokens.
const SECTION_CHARS: usize = 1200;
/// Sections a query may embed inline; a larger backlog continues on later
/// queries or with `index rebuild`, and search stays lexical meanwhile.
pub(crate) const INLINE_BUDGET: usize = 64;
// ponytail: fixed floors measured on invented notes (tests/semantic.rs):
// e5 similarities sit in a narrow band, so margins are about 0.02. Make them
// configurable if real vaults disagree.
/// Query to section or memory: relevant 0.81–0.83, unrelated 0.74–0.79.
const QUERY_FLOOR: f32 = 0.80;
/// Note to note or memory to memory: similar 0.857–0.863, unrelated ≤ 0.828.
pub(crate) const PAIR_FLOOR: f32 = 0.845;
/// Semantic candidates kept per query before fusion. Short queries embed
/// generically and clear the floor broadly, so only the closest few vote.
const CANDIDATES: usize = 10;

pub(crate) struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
}

// One model per process, memory-mapped so concurrent servers share pages.
static LOADED: Mutex<Option<(PathBuf, Arc<Embedder>)>> = Mutex::new(None);

impl Embedder {
    fn load(dir: &Path) -> Result<Self> {
        let config: Config = serde_json::from_str(&fs::read_to_string(dir.join("config.json"))?)?;
        let mut tokenizer =
            Tokenizer::from_file(dir.join("tokenizer.json")).map_err(anyhow::Error::msg)?;
        let pad_id = tokenizer
            .token_to_id("<pad>")
            .context("tokenizer lacks <pad>")?;
        tokenizer.with_padding(Some(PaddingParams {
            pad_id,
            pad_token: "<pad>".into(),
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 512,
                ..Default::default()
            }))
            .map_err(anyhow::Error::msg)?;
        // SAFETY: the file was checksum-verified at install into the private
        // state directory, and installs replace the directory instead of
        // rewriting files in place, so a mapped file is never mutated.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                &[dir.join("model.safetensors")],
                DType::F32,
                &Device::Cpu,
            )?
        };
        Ok(Self {
            model: BertModel::load(vb, &config)?,
            tokenizer,
        })
    }

    /// Mean-pooled, L2-normalized vectors (the e5 recipe).
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(16) {
            let encodings = self
                .tokenizer
                .encode_batch(batch.to_vec(), true)
                .map_err(anyhow::Error::msg)?;
            let stack = |f: fn(&tokenizers::Encoding) -> &[u32]| -> Result<Tensor> {
                let rows = encodings
                    .iter()
                    .map(|e| Tensor::new(f(e), &Device::Cpu))
                    .collect::<candle_core::Result<Vec<_>>>()?;
                Ok(Tensor::stack(&rows, 0)?)
            };
            let ids = stack(|e| e.get_ids())?;
            let mask = stack(|e| e.get_attention_mask())?;
            let hidden = self.model.forward(&ids, &ids.zeros_like()?, Some(&mask))?;
            let mask = mask.to_dtype(DType::F32)?.unsqueeze(2)?;
            let pooled = hidden
                .broadcast_mul(&mask)?
                .sum(1)?
                .broadcast_div(&mask.sum(1)?)?;
            let norm = pooled.sqr()?.sum_keepdim(1)?.sqrt()?;
            out.extend(pooled.broadcast_div(&norm)?.to_vec2::<f32>()?);
        }
        Ok(out)
    }
}

/// Heading-delimited sections of a note body, each prefixed with a label
/// (the title, else the path). Heading-only sections are skipped: they look
/// alike across notes and inflate similarity.
fn sections(label: &str, text: &str) -> Vec<String> {
    let body = Note::parse(text).map(|n| n.body()).unwrap_or(text);
    let mut bounds: Vec<usize> = Parser::new(body)
        .into_offset_iter()
        .filter_map(|(e, r)| matches!(e, Event::Start(Tag::Heading { .. })).then_some(r.start))
        .collect();
    bounds.insert(0, 0);
    bounds.push(body.len());
    let mut out = Vec::new();
    for pair in bounds.windows(2) {
        let section = &body[pair[0]..pair[1]];
        // Pieces of a long section each repeat its heading for context.
        let (heading, rest) = match section.split_once('\n') {
            Some((h, r)) if h.starts_with('#') => (h, r),
            _ if section.starts_with('#') => (section, ""),
            _ => ("", section),
        };
        let mut piece = String::new();
        for line in rest.lines() {
            // A single overlong line stays whole; the tokenizer truncates it.
            if !piece.trim().is_empty() && piece.len() + line.len() > SECTION_CHARS {
                out.push(format!("{heading}\n{}", std::mem::take(&mut piece)));
            }
            piece.push_str(line);
            piece.push('\n');
        }
        if !piece.trim().is_empty() {
            out.push(format!("{heading}\n{piece}"));
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out.into_iter()
        .map(|s| format!("passage: {label}\n{}", s.trim()))
        .collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Reciprocal rank fusion (k = 60) of best-first path lists.
pub(crate) fn fuse(lists: &[Vec<String>]) -> Vec<(String, f64)> {
    let mut scores = BTreeMap::<String, f64>::new();
    for list in lists {
        for (rank, path) in list.iter().enumerate() {
            *scores.entry(path.clone()).or_default() += 1.0 / (61.0 + rank as f64);
        }
    }
    let mut out: Vec<_> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

pub(crate) struct Vectors {
    revision: String,
    sections: Vec<Vec<f32>>,
    memory: Option<Vec<f32>>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    model: String,
    dimensions: usize,
    /// Path, revision, section count, has a memory vector.
    notes: Vec<(String, String, usize, bool)>,
}

/// `u64` header length, JSON header, then little-endian `f32` vectors.
fn encode(notes: &BTreeMap<String, Vectors>) -> Result<Vec<u8>> {
    let header = Header {
        model: format!("{MODEL}@{REVISION}"),
        dimensions: 384,
        notes: notes
            .iter()
            .map(|(p, v)| {
                (
                    p.clone(),
                    v.revision.clone(),
                    v.sections.len(),
                    v.memory.is_some(),
                )
            })
            .collect(),
    };
    let header = serde_json::to_vec(&header)?;
    let mut out = (header.len() as u64).to_le_bytes().to_vec();
    out.extend(header);
    for v in notes.values() {
        for x in v.sections.iter().chain(&v.memory).flatten() {
            out.extend(x.to_le_bytes());
        }
    }
    Ok(out)
}

/// Any mismatch yields `None`, and the cache is rebuilt.
fn decode(bytes: &[u8]) -> Option<BTreeMap<String, Vectors>> {
    let len = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?) as usize;
    let header: Header = serde_json::from_slice(bytes.get(8..8usize.checked_add(len)?)?).ok()?;
    if header.model != format!("{MODEL}@{REVISION}") || header.dimensions != 384 {
        return None;
    }
    let (words, rest) = bytes[8 + len..].as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    let mut floats = words.iter().map(|c| f32::from_le_bytes(*c));
    let mut take = || -> Option<Vec<f32>> {
        let v: Vec<f32> = floats.by_ref().take(384).collect();
        (v.len() == 384).then_some(v)
    };
    let mut notes = BTreeMap::new();
    for (path, revision, count, memory) in header.notes {
        let sections = (0..count).map(|_| take()).collect::<Option<Vec<_>>>()?;
        let memory = if memory { Some(take()?) } else { None };
        notes.insert(
            path,
            Vectors {
                revision,
                sections,
                memory,
            },
        );
    }
    floats.next().is_none().then_some(notes)
}

/// Streams the file: the model is too large to hash in one read.
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        match io::Read::read(&mut file, &mut buffer)? {
            0 => break,
            n => hasher.update(&buffer[..n]),
        }
    }
    Ok(note::hex(&hasher.finalize()))
}

pub(crate) struct Semantic {
    embedder: Arc<Embedder>,
    notes: BTreeMap<String, Vectors>,
}

impl Semantic {
    pub(crate) fn query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embedder.embed(&[format!("query: {text}")])?.remove(0))
    }

    /// Notes (or memory statements) closest to a query vector, best first.
    pub(crate) fn nearest(
        &self,
        query: &[f32],
        memories: bool,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<(String, f32)> {
        self.ranked(QUERY_FLOOR, keep, |v| {
            if memories {
                v.memory.as_deref().map(|m| dot(query, m))
            } else {
                v.sections.iter().map(|s| dot(query, s)).reduce(f32::max)
            }
        })
    }

    /// Notes whose closest sections resemble any section of `path`.
    pub(crate) fn similar(&self, path: &str, keep: impl Fn(&str) -> bool) -> Vec<(String, f32)> {
        let Some(own) = self.notes.get(path) else {
            return Vec::new();
        };
        self.ranked(
            PAIR_FLOOR,
            |p| p != path && keep(p),
            |v| {
                v.sections
                    .iter()
                    .flat_map(|s| own.sections.iter().map(|o| dot(o, s)))
                    .reduce(f32::max)
            },
        )
    }

    pub(crate) fn memory_similarity(&self, a: &str, b: &str) -> Option<f32> {
        let vector = |p: &str| self.notes.get(p)?.memory.as_deref();
        Some(dot(vector(a)?, vector(b)?))
    }

    fn ranked(
        &self,
        floor: f32,
        keep: impl Fn(&str) -> bool,
        score: impl Fn(&Vectors) -> Option<f32>,
    ) -> Vec<(String, f32)> {
        let mut out: Vec<_> = self
            .notes
            .iter()
            .filter(|(p, _)| keep(p))
            .filter_map(|(p, v)| Some((p.clone(), score(v)?)))
            .filter(|(_, s)| *s >= floor)
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(CANDIDATES);
        out
    }
}

impl Vault {
    fn model_dir(&self) -> PathBuf {
        self.state.join("model")
    }

    /// Download (or copy `from` a directory) the pinned model files, verify
    /// their SHA-256, then replace the installed model directory.
    pub fn install_model(&self, from: Option<&Path>) -> Result<Value> {
        let dir = self.model_dir();
        let present = FILES
            .iter()
            .all(|(f, h)| sha256_file(&dir.join(f)).is_ok_and(|x| x == *h));
        if !present {
            let staging = tempfile::tempdir_in(&self.state)?;
            for (file, expected) in FILES {
                let target = staging.path().join(file);
                match from {
                    Some(source) => {
                        fs::copy(source.join(file), &target).with_context(|| {
                            format!("cannot copy {file} from {}", source.display())
                        })?;
                    }
                    None => {
                        let url =
                            format!("https://huggingface.co/{MODEL}/resolve/{REVISION}/{file}");
                        let status = Command::new("curl")
                            .args(["-fsSL", "--proto", "=https", "--retry", "3", "-o"])
                            .arg(&target)
                            .arg(&url)
                            .status()
                            .context("curl is needed to download the model; or pass --from DIR")?;
                        ensure!(status.success(), "download failed: {url}");
                    }
                }
                let actual = sha256_file(&target)?;
                ensure!(
                    actual == expected,
                    "{file} checksum mismatch: expected {expected}, got {actual}; nothing was installed"
                );
            }
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
            fs::rename(staging.keep(), &dir)?;
            *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        Ok(
            json!({"model":MODEL,"revision":REVISION,"path":dir,"already_installed":present,"next":"Semantic search turns on automatically. Run `index rebuild` to embed the whole vault now; otherwise queries embed changed notes gradually."}),
        )
    }

    fn embedder(&self) -> Result<Option<Arc<Embedder>>> {
        let dir = self.model_dir();
        if !FILES.iter().all(|(f, _)| dir.join(f).is_file()) {
            return Ok(None);
        }
        let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((d, e)) = &*loaded
            && *d == dir
        {
            return Ok(Some(e.clone()));
        }
        let embedder =
            Arc::new(Embedder::load(&dir).context("cannot load the model; reinstall it")?);
        *loaded = Some((dir, embedder.clone()));
        Ok(Some(embedder))
    }

    /// Vectors for every indexed note, embedding at most `budget` sections
    /// now. `Ok(Err(reason))` means callers should stay lexical.
    pub(crate) fn semantic(
        &self,
        index: &Index,
        budget: Option<usize>,
    ) -> Result<std::result::Result<Semantic, String>> {
        let Some(embedder) = self.embedder()? else {
            return Ok(Err(
                "Semantic search is off; install the model with `mcp-brain model install`.".into(),
            ));
        };
        let mut notes = self
            .read_state_bytes(STORE)?
            .and_then(|b| decode(&b))
            .unwrap_or_default();
        let before = notes.len();
        notes.retain(|p, v| {
            index
                .documents
                .get(p)
                .is_some_and(|d| d.revision == v.revision)
        });
        let mut changed = notes.len() != before;
        let pending: Vec<_> = index
            .documents
            .iter()
            .filter(|(p, _)| !notes.contains_key(*p))
            .collect();
        let mut spent = 0;
        let mut left = 0;
        for (done, (path, doc)) in pending.iter().enumerate() {
            let text = self.read_text(path)?;
            let mut texts = sections(doc.title.as_deref().unwrap_or(path), &text);
            if let Some(m) = &doc.memory {
                texts.push(format!("passage: {}\n{}", m.title, m.content));
            }
            if budget.is_some_and(|b| spent + texts.len() > b) {
                left += texts.len();
                continue;
            }
            spent += texts.len();
            let mut vectors = embedder.embed(&texts)?;
            let memory = doc.memory.as_ref().and_then(|_| vectors.pop());
            notes.insert(
                (*path).clone(),
                Vectors {
                    revision: note::revision(&text),
                    sections: vectors,
                    memory,
                },
            );
            changed = true;
            if budget.is_none() && (done + 1) % 50 == 0 {
                eprintln!("embedded {}/{} notes", done + 1, pending.len());
            }
        }
        if changed {
            self.write_state_bytes(STORE, &encode(&notes)?)?;
        }
        if left > 0 {
            return Ok(Err(format!(
                "Semantic search is catching up: {left} sections await embedding. Run `mcp-brain index rebuild` to finish now."
            )));
        }
        Ok(Ok(Semantic { embedder, notes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_split_on_headings_and_length() {
        let long = "word ".repeat(400);
        let text = format!("---\nauthor: ai\n---\nIntro\n# One\nalpha\n## Two\n{long}\n{long}\n");
        let s = sections("A.md", &text);
        assert_eq!(s.len(), 4, "{s:?}");
        assert!(s.iter().all(|x| x.starts_with("passage: A.md\n")));
        assert!(!s[0].contains("author"), "frontmatter is not embedded");
        assert!(s[1].contains("# One") && s[1].contains("alpha"));
        assert!(s[2].contains("## Two\nword") && s[3].contains("## Two\nword"));
        assert_eq!(sections("Empty.md", ""), ["passage: Empty.md\n"]);
    }

    #[test]
    fn store_round_trips_and_rejects_damage() {
        let v = |x: f32| vec![x; 384];
        let mut notes = BTreeMap::new();
        notes.insert(
            "A.md".to_owned(),
            Vectors {
                revision: "r1".into(),
                sections: vec![v(0.5), v(-1.0)],
                memory: Some(v(2.0)),
            },
        );
        notes.insert(
            "B.md".to_owned(),
            Vectors {
                revision: "r2".into(),
                sections: vec![v(3.0)],
                memory: None,
            },
        );
        let bytes = encode(&notes).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back["A.md"].sections[1], v(-1.0));
        assert_eq!(back["A.md"].memory, Some(v(2.0)));
        assert_eq!(back["B.md"].revision, "r2");
        assert!(decode(&bytes[..bytes.len() - 4]).is_none());
        assert!(decode(b"garbage").is_none());
    }

    #[test]
    fn fusion_rewards_agreement() {
        let list = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let fused = fuse(&[list(&["a", "b", "c"]), list(&["b", "x"])]);
        assert_eq!(fused[0].0, "b");
        assert_eq!(fused.last().unwrap().0, "c");
    }
}
