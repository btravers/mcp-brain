use crate::{
    git,
    note::{self, Author, Note},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const PARA: [(&str, &str, &str); 6] = [
    (
        "inbox",
        "0 Inbox",
        "Unsorted captures; review before choosing a destination.",
    ),
    (
        "projects",
        "1 Projects",
        "A concrete goal and an end date; done is answerable.",
    ),
    (
        "areas",
        "2 Areas",
        "An ongoing responsibility without an end date.",
    ),
    (
        "resources",
        "3 Resources",
        "Reference or interests without an ongoing obligation.",
    ),
    (
        "archive",
        "4 Archive",
        "Completed, dropped, or inactive material.",
    ),
    (
        "meta",
        "5 Meta",
        "Templates, daily notes, and vault plumbing.",
    ),
];

#[derive(Debug, Clone)]
pub struct Vault {
    pub root: PathBuf,
    pub state: PathBuf,
}

pub struct VaultGuard(File);

impl Drop for VaultGuard {
    fn drop(&mut self) {
        // Explicitly unlock before closing. A concurrent subprocess launch may
        // briefly inherit the descriptor across fork, delaying a close-only unlock.
        let _ = self.0.unlock();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub changed: Vec<String>,
    pub revisions: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub fn category(path: &str) -> Option<&'static str> {
    let first = path.split('/').next()?;
    PARA.iter()
        .find(|(_, folder, _)| *folder == first)
        .map(|(key, _, _)| *key)
}

pub fn valid_relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 1024,
        "path must contain 1–1024 bytes"
    );
    ensure!(
        !path.contains(['\\', ':', '*', '?', '"', '<', '>', '|', '#', '[', ']', '^'])
            && !path.chars().any(char::is_control),
        "use a portable vault-relative path with forward slashes"
    );
    for part in path.split('/') {
        ensure!(
            !part.is_empty() && !part.starts_with('.') && !part.ends_with(['.', ' ']),
            "hidden paths, traversal, and empty components are forbidden"
        );
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            !matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ),
            "reserved Windows filename"
        );
    }
    Ok(())
}

fn reject_alias(meta: &fs::Metadata) -> Result<()> {
    ensure!(!meta.file_type().is_symlink(), "symlinks are forbidden");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "Windows reparse points are forbidden"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            !meta.is_file() || meta.nlink() == 1,
            "hard-linked notes are forbidden"
        );
    }
    Ok(())
}

impl Vault {
    pub fn open(root: impl AsRef<Path>, state: Option<PathBuf>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("vault directory does not exist")?;
        ensure!(root.is_dir(), "vault must be a directory");
        let state = match state {
            Some(p) => p,
            None => directories::ProjectDirs::from("", "", "mcp-brain")
                .context("cannot locate local application data directory; pass --state-dir")?
                .data_local_dir()
                .join(&note::revision(&root.to_string_lossy())[..24]),
        };
        fs::create_dir_all(&state)?;
        let state = state.canonicalize()?;
        ensure!(
            !state.starts_with(&root),
            "state directory must be outside the vault"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { root, state })
    }

    pub fn lock(&self) -> Result<VaultGuard> {
        let path = if git::enabled(&self.root) {
            PathBuf::from(git::git(
                &self.root,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "mcp-brain.lock",
                ],
            )?)
        } else {
            self.state.join("vault.lock")
        };
        if let Ok(meta) = fs::symlink_metadata(&path) {
            reject_alias(&meta)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock()
            .context("another mcp-brain operation is using this vault; retry shortly")?;
        Ok(VaultGuard(file))
    }

    pub fn path(&self, relative: &str) -> Result<PathBuf> {
        valid_relative(relative)?;
        let mut path = self.root.clone();
        for part in relative.split('/') {
            // Keep one spelling across case-sensitive and case-insensitive
            // hosts. A second spelling must never become a second planned file.
            if path.is_dir() {
                for sibling in fs::read_dir(&path)? {
                    let sibling = sibling?;
                    let name = sibling.file_name();
                    let name = name.to_string_lossy();
                    ensure!(
                        name == part || name.to_lowercase() != part.to_lowercase(),
                        "path casing differs from an existing entry; use its exact spelling: {name}"
                    );
                }
            }
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) => reject_alias(&meta)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }

    pub fn note_path(&self, relative: &str) -> Result<PathBuf> {
        ensure!(relative.ends_with(".md"), "only .md notes are supported");
        ensure!(
            !matches!(
                relative.to_ascii_lowercase().as_str(),
                "agents.md" | "claude.md" | "gemini.md"
            ),
            "instruction files are available through vault_context, not note tools"
        );
        self.path(relative)
    }

    pub fn read_text(&self, relative: &str) -> Result<String> {
        let path = self.note_path(relative)?;
        self.read_bounded(&path)
    }

    fn read_bounded(&self, path: &Path) -> Result<String> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let meta = file.metadata()?;
        reject_alias(&meta)?;
        ensure!(meta.is_file(), "expected a regular file");
        ensure!(
            meta.len() <= note::MAX_NOTE_BYTES as u64,
            "note exceeds the 2 MiB limit"
        );
        let mut text = String::new();
        file.take(note::MAX_NOTE_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .context("notes must be UTF-8")?;
        ensure!(
            text.len() <= note::MAX_NOTE_BYTES,
            "note exceeds the 2 MiB limit"
        );
        Ok(text)
    }

    pub fn read(&self, path: &str) -> Result<Value> {
        let text = self.read_text(path)?;
        let parsed = Note::parse(&text);
        Ok(
            json!({"path":path, "revision": note::revision(&text), "content": text,
            "author": parsed.as_ref().map(|n| n.author).unwrap_or(Author::Human),
            "parse_error": parsed.err().map(|e| e.to_string()), "category":category(path)}),
        )
    }

    pub fn paths(&self) -> Result<Vec<String>> {
        let mut paths = Vec::new();
        for item in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        {
            let entry = item?;
            if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "md") {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)?
                .to_str()
                .context("non-UTF-8 path")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if self.note_path(&relative).is_ok() {
                paths.push(relative);
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn list(
        &self,
        area: Option<&str>,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        ensure!(
            (1..=200).contains(&limit),
            "limit must be between 1 and 200"
        );
        if let Some(area) = area {
            ensure!(
                PARA.iter().any(|(k, _, _)| *k == area),
                "unknown PARA category"
            );
        }
        let query = query.map(str::to_lowercase);
        let mut found = Vec::new();
        for path in self.paths()? {
            if area.is_some_and(|a| category(&path) != Some(a)) {
                continue;
            }
            let text = self.read_text(&path)?;
            if query.as_ref().is_some_and(|q| {
                !path.to_lowercase().contains(q) && !text.to_lowercase().contains(q)
            }) {
                continue;
            }
            let parsed = Note::parse(&text);
            found.push(
                json!({"path":path, "category":category(&path), "revision":note::revision(&text),
                "title":parsed.as_ref().ok().and_then(|n|n.title()),
                "author":parsed.as_ref().map(|n|n.author).unwrap_or(Author::Human),
                "parse_error":parsed.err().map(|e|e.to_string())}),
            );
        }
        let total = found.len();
        Ok(
            json!({"total":total,"offset":offset,"notes":found.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"next_offset": if offset.saturating_add(limit)<total {Some(offset+limit)} else {None}}),
        )
    }

    pub fn context(&self) -> Result<Value> {
        let paths = self.paths()?;
        let categories: Vec<_> = PARA.iter().map(|(key, folder, rule)| json!({"category":key,"folder":folder,"rule":rule,"count":paths.iter().filter(|p| category(p)==Some(key)).count()})).collect();
        let rules_path = self.path("AGENTS.md")?;
        let rules = if rules_path.exists() {
            Some(self.read_bounded(&rules_path)?)
        } else {
            None
        };
        Ok(
            json!({"categories":categories, "vault_rules":rules, "git_backup":git::enabled(&self.root), "synchronization":self.sync_status()?,
            "ownership":"Human notes are immutable except a marked trailing AI section and additive up metadata. Missing author means human. AI notes retain their trailing Notes section.",
            "privacy":"Never store credentials or private client material. The server cannot determine whether arbitrary prose contains personal or confidential data.",
            "inbox":self.inbox_status()?}),
        )
    }

    pub fn snapshot(&self) -> Result<BTreeMap<String, String>> {
        self.paths()?
            .into_iter()
            .map(|p| Ok((p.clone(), note::revision(&self.read_text(&p)?))))
            .collect()
    }

    pub fn write_state(&self, filename: &str, content: &str) -> Result<()> {
        valid_relative(filename)?;
        ensure!(
            !filename.contains('/'),
            "state filename cannot contain directories"
        );
        let path = self.state.join(filename);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            reject_alias(&meta)?;
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.state)?;
        file.write_all(content.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn read_state(&self, filename: &str) -> Result<Option<String>> {
        valid_relative(filename)?;
        ensure!(
            !filename.contains('/'),
            "state filename cannot contain directories"
        );
        let path = self.state.join(filename);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                reject_alias(&meta)?;
                Ok(Some(fs::read_to_string(path)?))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Validate every change before writing any file. A recovery journal keeps
    /// the originals if a filesystem or Git operation fails mid-transaction.
    pub fn apply(&self, changes: &[Change], message: &str) -> Result<Outcome> {
        self.require_recovered()?;
        let changes: Vec<_> = changes.iter().filter(|c| c.before != c.after).collect();
        let mut seen = BTreeSet::new();
        for change in &changes {
            ensure!(seen.insert(&change.path), "duplicate change path");
            let path = self.note_path(&change.path)?;
            let actual = if path.exists() {
                Some(self.read_text(&change.path)?)
            } else {
                None
            };
            ensure!(
                actual == change.before,
                "{} changed; regenerate the operation",
                change.path
            );
            if let Some(after) = &change.after {
                Note::parse(after)?;
            }
        }
        if changes.is_empty() {
            return Ok(Outcome {
                changed: vec![],
                revisions: BTreeMap::new(),
                warning: None,
            });
        }
        git::validate_snapshots(&self.root, &changes)?;
        git::checkpoint(&self.root, self.policy()?.offline_writes)?;
        // Git hooks or concurrent editors can have changed files during checkpoint.
        for change in &changes {
            let path = self.note_path(&change.path)?;
            let actual = if path.exists() {
                Some(self.read_text(&change.path)?)
            } else {
                None
            };
            ensure!(
                actual == change.before,
                "{} changed during checkpoint; regenerate the operation",
                change.path
            );
        }
        let journal = format!("transaction-{}.json", uuid::Uuid::new_v4());
        self.write_state(&journal, &serde_json::to_string_pretty(&changes)?)?;
        self.write_changes(&changes, &journal)?;
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        let owned: Vec<Change> = changes.iter().map(|c| (*c).clone()).collect();
        let warning = match git::finish(&self.root, &owned, message) {
            Ok(()) => {
                self.mark_pending_push()?;
                fs::remove_file(self.state.join(&journal))?;
                self.backup()?
            }
            Err(e) => Some(format!(
                "Notes were saved but commit is incomplete: {e:#}. Do not repeat the edit; run recover show/finish {journal}."
            )),
        };
        Ok(Outcome {
            changed: paths,
            revisions: changes
                .iter()
                .filter_map(|c| {
                    c.after
                        .as_ref()
                        .map(|a| (c.path.clone(), note::revision(a)))
                })
                .collect(),
            warning,
        })
    }

    pub(crate) fn write_changes(&self, changes: &[&Change], journal: &str) -> Result<()> {
        // Write all destinations before removing any moved sources.
        for change in changes.iter().filter(|c| c.after.is_some()) {
            let path = self.note_path(&change.path)?;
            let parent = path.parent().context("missing parent")?;
            fs::create_dir_all(parent)?;
            let path = self.note_path(&change.path)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            if let Ok(meta) = fs::metadata(&path) {
                temp.as_file().set_permissions(meta.permissions())?;
            }
            temp.write_all(change.after.as_ref().unwrap().as_bytes())?;
            temp.as_file().sync_all()?;
            if let Some(before) = &change.before {
                ensure!(
                    self.read_text(&change.path)? == *before,
                    "file changed while writing; recovery journal: {journal}"
                );
                temp.persist(&path)
                    .map_err(|e| e.error)
                    .with_context(|| format!("write failed; recovery journal: {journal}"))?;
            } else {
                temp.persist_noclobber(&path)
                    .map_err(|e| e.error)
                    .with_context(|| format!("write failed; recovery journal: {journal}"))?;
            }
        }
        for change in changes.iter().filter(|c| c.after.is_none()) {
            ensure!(
                self.read_text(&change.path)?
                    == *change.before.as_ref().context("missing move source")?,
                "move source changed; recovery journal: {journal}"
            );
            fs::remove_file(self.note_path(&change.path)?)?;
        }
        Ok(())
    }

    pub fn create(&self, filename: &str, title: &str, body: &str) -> Result<Outcome> {
        let _lock = self.lock()?;
        ensure!(
            !filename.contains('/') && filename.ends_with(".md"),
            "filename must be a single .md name; new notes go to Inbox"
        );
        let path = format!("0 Inbox/{filename}");
        ensure!(!self.note_path(&path)?.exists(), "note already exists");
        let paths = self.paths()?;
        ensure!(
            !paths.iter().any(|p| p
                .rsplit('/')
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case(filename))),
            "a note with this filename already exists; choose an unambiguous name"
        );
        ensure!(
            filename != "0 Inbox.md",
            "the folder index is managed by the server"
        );
        let mut changes = vec![Change {
            path,
            before: None,
            after: Some(note::new_ai_note(title, body)?),
        }];
        self.maintain_indexes(&mut changes, &["0 Inbox".into()])?;
        self.apply(&changes, "capture note")
    }

    pub fn edit(&self, path: &str, revision: &str, body: &str, mode: &str) -> Result<Outcome> {
        let _lock = self.lock()?;
        let before = self.read_text(path)?;
        let note = Note::parse(&before)?;
        note.check_revision(revision)?;
        ensure!(
            mode != "body" || !crate::memory::is_memory(&note),
            "use update_memory for memory records so history and provenance are preserved"
        );
        let after = match mode {
            "body" => note.replace_ai_body(body)?,
            "append" => note.set_ai_section(body, true)?,
            "section" => note.set_ai_section(body, false)?,
            _ => bail!("unknown edit mode"),
        };
        self.apply(
            &[Change {
                path: path.into(),
                before: Some(before),
                after: Some(after),
            }],
            "update AI content",
        )
    }

    pub fn maintain_indexes(&self, changes: &mut Vec<Change>, folders: &[String]) -> Result<()> {
        let mut virtual_files: BTreeMap<String, String> = self
            .paths()?
            .into_iter()
            .map(|p| Ok((p.clone(), self.read_text(&p)?)))
            .collect::<Result<_>>()?;
        for c in changes.iter() {
            if let Some(after) = &c.after {
                virtual_files.insert(c.path.clone(), after.clone());
            } else {
                virtual_files.remove(&c.path);
            }
        }
        for folder in folders.iter().collect::<BTreeSet<_>>() {
            let name = folder.rsplit('/').next().context("empty folder")?;
            let index = format!("{folder}/{name}.md");
            let children: Vec<String> = virtual_files
                .keys()
                .filter(|p| {
                    p.rsplit_once('/')
                        .is_some_and(|(parent, _)| parent == folder.as_str())
                        && **p != index
                })
                .cloned()
                .collect();
            if children.len() < 2 && !virtual_files.contains_key(&index) {
                continue;
            }
            let mut links = Vec::new();
            for child in &children {
                let text = virtual_files.get(child).unwrap();
                let updated = Note::parse(text)?.add_up(name)?;
                virtual_files.insert(child.clone(), updated);
                links.push(format!("- [[{}]]", child.strip_suffix(".md").unwrap()));
            }
            let links = links.join("\n");
            let text = match virtual_files.get(&index) {
                None => note::new_ai_note(name, &links)?,
                Some(text) => {
                    let parsed = Note::parse(text)?;
                    ensure!(
                        parsed.title().as_deref() == Some(name),
                        "folder index heading must match {name}; the human must repair it"
                    );
                    if parsed.author == Author::Human {
                        let old = parsed.ai_section_content()?.unwrap_or("");
                        let missing: Vec<_> =
                            links.lines().filter(|line| !text.contains(*line)).collect();
                        if missing.is_empty() {
                            text.clone()
                        } else {
                            parsed
                                .set_ai_section(&format!("{old}\n{}", missing.join("\n")), false)?
                        }
                    } else {
                        let end = parsed.protected_notes_start()?.unwrap_or(text.len());
                        let body = &text[parsed.body_start..end];
                        let missing: Vec<_> =
                            links.lines().filter(|line| !body.contains(*line)).collect();
                        if missing.is_empty() {
                            text.clone()
                        } else {
                            parsed.replace_ai_body(&format!("{body}\n{}", missing.join("\n")))?
                        }
                    }
                }
            };
            virtual_files.insert(index, text);
        }
        for (path, after) in virtual_files {
            if let Some(c) = changes.iter_mut().find(|c| c.path == path) {
                c.after = Some(after);
            } else {
                let before = if self.note_path(&path)?.exists() {
                    Some(self.read_text(&path)?)
                } else {
                    None
                };
                if before.as_ref() != Some(&after) {
                    changes.push(Change {
                        path,
                        before,
                        after: Some(after),
                    });
                }
            }
        }
        Ok(())
    }
}
