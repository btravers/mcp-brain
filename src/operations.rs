//! Local policy, synchronization, recovery, and optional foreground automation.
use crate::{
    git, note,
    vault::{Change, PARA, Vault},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    time::{Duration, Instant},
};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Local opt-in. MCP clients cannot change this setting.
    pub offline_writes: bool,
    pub sync_on_start: bool,
    pub sync_interval_seconds: Option<u64>,
}

impl Vault {
    pub fn policy(&self) -> Result<Policy> {
        Ok(self
            .read_state("policy.json")?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default())
    }
    pub fn configure(&self, policy: Policy) -> Result<Value> {
        let _lock = self.lock()?;
        ensure!(
            policy.sync_interval_seconds.is_none_or(|n| n >= 30),
            "sync interval must be at least 30 seconds"
        );
        self.write_state("policy.json", &serde_json::to_string_pretty(&policy)?)?;
        Ok(serde_json::to_value(policy)?)
    }
    pub fn journals(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.state)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("transaction-") && name.ends_with(".json") {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
    pub(crate) fn require_recovered(&self) -> Result<()> {
        ensure!(
            self.journals()?.is_empty(),
            "an unfinished transaction needs recovery first; run recover show <journal> and recover finish <journal>"
        );
        Ok(())
    }
    pub fn sync_status(&self) -> Result<Value> {
        Ok(
            json!({"git":git::status(&self.root)?,"policy":self.policy()?,"last_sync":self.read_state("sync.json")?.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"last_sync_error":self.read_state("sync-error.json")?.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"pending_push":self.read_state("pending-push.json")?.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"pending_transactions":self.journals()?}),
        )
    }
    pub fn synchronize(&self) -> Result<Value> {
        let _lock = self.lock()?;
        self.require_recovered()?;
        match git::sync(&self.root) {
            Ok(()) => {
                self.clear_pending_push()?;
                if self.read_state("sync-error.json")?.is_some() {
                    fs::remove_file(self.state.join("sync-error.json"))?;
                }
                self.write_state(
                    "sync.json",
                    &json!({"completed_at":chrono::Utc::now().to_rfc3339()}).to_string(),
                )?;
            }
            Err(error) => {
                self.write_state(
                    "sync-error.json",
                    &json!({"at":chrono::Utc::now().to_rfc3339(),"error":error.to_string()})
                        .to_string(),
                )?;
                return Err(error);
            }
        }
        self.sync_status()
    }
    fn clear_pending_push(&self) -> Result<()> {
        if self.read_state("pending-push.json")?.is_some() {
            fs::remove_file(self.state.join("pending-push.json"))?;
        }
        Ok(())
    }
    pub(crate) fn mark_pending_push(&self) -> Result<()> {
        if git::enabled(&self.root) {
            self.write_state("pending-push.json",&json!({"since":chrono::Utc::now().to_rfc3339(),"state":"local commit awaiting upload"}).to_string())?;
        }
        Ok(())
    }
    pub(crate) fn backup(&self) -> Result<Option<String>> {
        match git::push(&self.root) {
            Ok(()) => {
                self.clear_pending_push()?;
                Ok(None)
            }
            Err(e) => {
                let warning = format!(
                    "Notes are committed locally; push is pending: {e:#}. Run recover push; do not repeat the edit."
                );
                self.write_state(
                    "pending-push.json",
                    &json!({"since":chrono::Utc::now().to_rfc3339(),"error":format!("{e:#}")})
                        .to_string(),
                )?;
                Ok(Some(warning))
            }
        }
    }
    pub fn retry_push(&self) -> Result<Value> {
        let _lock = self.lock()?;
        self.require_recovered()?;
        let warning = self.backup()?;
        Ok(json!({"warning":warning,"status":self.sync_status()?}))
    }
    pub fn recovery_show(&self, name: &str) -> Result<Vec<Change>> {
        ensure!(
            self.journals()?.iter().any(|n| n == name),
            "unknown transaction journal"
        );
        serde_json::from_str(&self.read_state(name)?.context("missing transaction")?)
            .context("invalid transaction journal")
    }
    /// Recovery never rolls back human edits. Every path must still match one
    /// of the two recorded states. The CLI requires local terminal approval.
    pub fn recovery_finish(&self, name: &str) -> Result<Value> {
        let _lock = self.lock()?;
        let changes = self.recovery_show(name)?;
        let mut remaining = Vec::new();
        for c in &changes {
            let path = self.note_path(&c.path)?;
            let actual = if path.exists() {
                Some(self.read_text(&c.path)?)
            } else {
                None
            };
            ensure!(
                actual == c.before || actual == c.after,
                "{} has later edits; preserve them and reconcile manually with the journal",
                c.path
            );
            if actual != c.after {
                remaining.push(c);
            }
        }
        git::validate_snapshots(&self.root, &changes.iter().collect::<Vec<_>>())?;
        self.write_changes(&remaining, name)?;
        git::finish(&self.root, &changes, "recover interrupted operation")?;
        self.mark_pending_push()?;
        // File and commit recovery is now complete, regardless of connectivity.
        fs::remove_file(self.state.join(name))?;
        let warning = self.backup()?;
        Ok(json!({"recovered":name,"warning":warning}))
    }
    pub fn watch_once(&self) -> Result<Value> {
        let _lock = self.lock()?;
        self.require_recovered()?;
        ensure!(
            git::enabled(&self.root),
            "watch requires a Git vault and configured upstream"
        );
        // Human saves are a distinct checkpoint, never an ai: commit.
        git::checkpoint(&self.root, true)?;
        let warning = self.backup()?;
        Ok(json!({"checkpointed":true,"warning":warning}))
    }
    pub fn watch_fingerprint(&self) -> Result<String> {
        let mut fingerprint = git::git(
            &self.root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        // Hash changed files, not merely status flags: repeated saves of an
        // already-modified file must restart the debounce timer.
        let tracked = git::git(
            &self.root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        )?;
        for name in tracked.split('\0').filter(|s| !s.is_empty()) {
            let path = self.root.join(name);
            if let Ok(meta) = fs::symlink_metadata(&path)
                && meta.is_file()
                && !meta.file_type().is_symlink()
            {
                use sha2::{Digest, Sha256};
                let mut file = fs::File::open(path)?;
                let mut hash = Sha256::new();
                std::io::copy(&mut file, &mut hash)?;
                fingerprint.push_str(&format!("{name}:{:x}", hash.finalize()));
            }
        }
        Ok(note::revision(&fingerprint))
    }
    /// Blocking foreground process. Ctrl-C terminates it; no daemon is installed.
    pub fn watch(&self, debounce_seconds: u64) -> Result<()> {
        ensure!(
            (1..=3600).contains(&debounce_seconds),
            "debounce must be 1–3600 seconds"
        );
        ensure!(git::enabled(&self.root), "watch requires Git");
        let policy = self.policy()?;
        if policy.sync_on_start {
            self.synchronize()?;
        }
        let mut previous = self.watch_fingerprint()?;
        let mut changed_at = Instant::now();
        let mut sync_at = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let current = self.watch_fingerprint()?;
            if current != previous {
                previous = current;
                changed_at = Instant::now();
            }
            if changed_at.elapsed() >= Duration::from_secs(debounce_seconds) {
                if git::dirty(&self.root)? || self.read_state("pending-push.json")?.is_some() {
                    match self.watch_once() {
                        Ok(v) => eprintln!("{v}"),
                        Err(e) => eprintln!("watch: {e:#}"),
                    }
                    previous = self.watch_fingerprint()?;
                }
                changed_at = Instant::now();
            }
            if policy
                .sync_interval_seconds
                .is_some_and(|n| sync_at.elapsed() >= Duration::from_secs(n))
            {
                if let Err(e) = self.synchronize() {
                    eprintln!("sync: {e:#}");
                }
                sync_at = Instant::now();
            }
        }
    }
    pub fn initialize(&self) -> Result<Value> {
        let _lock = self.lock()?;
        for (_, folder, _) in PARA {
            fs::create_dir_all(self.path(folder)?)?;
        }
        // Configuration examples are returned, never installed into client files.
        Ok(
            json!({"created":"PARA directories (existing notes preserved)","claude_code":{"mcpServers":{"brain":{"command":"mcp-brain","args":["--vault",self.root,"--state-dir",self.state,"serve"]}}},"codex":{"mcp_servers":{"brain":{"command":"mcp-brain","args":["--vault",self.root,"--state-dir",self.state,"serve"]}}},"reminder":"Optional: reminder configure --every-days 7; import reminder calendar output into your calendar for notifications.","next":"Run doctor; configure a private Git upstream yourself if desired. No Git remote or client files were changed."}),
        )
    }
    pub fn doctor(&self) -> Result<Value> {
        let mut issues = Vec::new();
        if let Err(e) = self.lock() {
            issues.push(format!("coordinator: {e}"));
        }
        let sync = match self.sync_status() {
            Ok(v) => v,
            Err(e) => {
                issues.push(format!("Git/config: {e}"));
                Value::Null
            }
        };
        if !self.journals()?.is_empty() {
            issues.push("unfinished transaction: inspect recover show".into());
        }
        if git::enabled(&self.root) && git::upstream(&self.root).is_err() {
            issues.push("configure a private backup upstream before writes".into());
        }
        let mut malformed = 0;
        for path in self.paths()? {
            if note::Note::parse(&self.read_text(&path)?).is_err() {
                malformed += 1;
            }
        }
        if malformed > 0 {
            issues.push(format!(
                "{malformed} Markdown notes have invalid frontmatter"
            ));
        }
        Ok(
            json!({"ok":issues.is_empty(),"version":env!("CARGO_PKG_VERSION"),"issues":issues,"sync":sync,"index":"Rebuildable local cache; index rebuild repairs it. Queries check content hashes.","limits":"No network authentication check performed. Restrict assistants' direct filesystem access separately. Use one Git automation service."}),
        )
    }
}
