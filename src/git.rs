//! Git operations share the vault coordinator. AI commits use exact snapshots,
//! never a second read of files that an editor may already have changed.
use crate::vault::Change;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn run(root: &Path, args: &[&str], index: Option<&Path>, input: Option<&[u8]>) -> Result<String> {
    // Files instead of pipes: a stalled SSH child cannot block pipe readers
    // after the Git process has been killed at the deadline.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut stdin = tempfile::tempfile()?;
    if let Some(bytes) = input {
        stdin.write_all(bytes)?;
        stdin.rewind()?;
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    // Do not inherit an unrelated shell's repository/index overrides.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(key);
    }
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=10",
        );
    }
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let mut child = command
        .spawn()
        .context("could not run git; install Git to use backup features")?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "git {} timed out after 30 seconds; inspect Git state before retrying",
                args.first().unwrap_or(&"operation")
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    stdout.rewind()?;
    stderr.rewind()?;
    let mut output = String::new();
    let mut error = String::new();
    stdout.read_to_string(&mut output)?;
    stderr.read_to_string(&mut error)?;
    ensure!(
        status.success(),
        "git {} failed: {}",
        args.first().unwrap_or(&"operation"),
        error.trim()
    );
    Ok(output.trim_end_matches('\n').trim_end_matches('\r').into())
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String> {
    run(root, args, None, None)
}
pub fn enabled(root: &Path) -> bool {
    root.join(".git").exists()
}

pub fn upstream(root: &Path) -> Result<String> {
    git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .context("Git vault needs a configured backup upstream before MCP writes")
}

fn ready(root: &Path) -> Result<()> {
    git(root, &["symbolic-ref", "-q", "HEAD"]).context("detached HEAD; select a branch first")?;
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        let path = git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        )?;
        ensure!(
            !Path::new(&path).exists(),
            "finish the in-progress Git operation before using mcp-brain"
        );
    }
    ensure!(
        git(root, &["ls-files", "--unmerged"])?.is_empty(),
        "resolve Git conflicts first"
    );
    Ok(())
}

pub fn dirty(root: &Path) -> Result<bool> {
    Ok(!git(root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
}

pub fn validate_snapshots(root: &Path, changes: &[&Change]) -> Result<()> {
    if !enabled(root) {
        return Ok(());
    }
    for change in changes {
        if let Some(after) = &change.after {
            let raw = run(
                root,
                &["hash-object", "--stdin"],
                None,
                Some(after.as_bytes()),
            )?;
            let converted = run(
                root,
                &["hash-object", &format!("--path={}", change.path), "--stdin"],
                None,
                Some(after.as_bytes()),
            )?;
            ensure!(
                raw == converted,
                "Git content conversion would alter planned bytes for {}; disable clean filters/line-ending conversion for Markdown, then retry",
                change.path
            );
        }
    }
    Ok(())
}

pub fn checkpoint(root: &Path, offline: bool) -> Result<()> {
    if !enabled(root) {
        return Ok(());
    }
    ready(root)?;
    upstream(root)?;
    if dirty(root)? {
        git(root, &["add", "--all"])?;
        git(root, &["commit", "-m", "Save existing vault changes"])?;
    }
    if !offline {
        push(root)?;
    }
    Ok(())
}

pub fn push(root: &Path) -> Result<()> {
    if enabled(root) {
        upstream(root)?;
        let branch = git(root, &["symbolic-ref", "--short", "HEAD"])?;
        let remote = git(
            root,
            &["config", "--get", &format!("branch.{branch}.remote")],
        )?;
        let destination = git(
            root,
            &["config", "--get", &format!("branch.{branch}.merge")],
        )?;
        ensure!(
            destination.starts_with("refs/heads/"),
            "backup upstream must be a branch"
        );
        git(
            root,
            &["push", "--", &remote, &format!("HEAD:{destination}")],
        )?;
    }
    Ok(())
}

/// A real Git index lock also excludes ordinary git add/commit while we update
/// HEAD and index. It does not prevent editors from saving working-tree files.
struct IndexLock(std::path::PathBuf);
impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn finish(root: &Path, changes: &[Change], message: &str) -> Result<()> {
    if !enabled(root) {
        return Ok(());
    }
    ready(root)?;
    let index = std::path::PathBuf::from(git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?);
    let lock_path = index.with_extension("lock");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .context("Git index is locked; stop other Git writers and retry recovery")?;
    let _lock = IndexLock(lock_path);
    let head = git(root, &["rev-parse", "HEAD"])?;
    let branch = git(root, &["symbolic-ref", "HEAD"])?;
    let temp = tempfile::tempdir_in(index.parent().context("index has no parent")?)?;
    let snapshot = temp.path().join("snapshot");
    let live = temp.path().join("live");
    fs::copy(&index, &live)?;
    run(root, &["read-tree", &head], Some(&snapshot), None)?;
    for change in changes {
        let original = run(
            root,
            &["ls-files", "--stage", "--", &change.path],
            Some(&snapshot),
            None,
        )?;
        let staged = run(
            root,
            &["ls-files", "--stage", "--", &change.path],
            Some(&live),
            None,
        )?;
        let mode = if original.starts_with("100755 ") {
            "100755"
        } else {
            "100644"
        };
        let blob = change
            .after
            .as_ref()
            .map(|after| {
                run(
                    root,
                    &["hash-object", "-w", "--stdin"],
                    None,
                    Some(after.as_bytes()),
                )
            })
            .transpose()?;
        let before_blob = change
            .before
            .as_ref()
            .map(|before| {
                run(
                    root,
                    &["hash-object", "--stdin"],
                    None,
                    Some(before.as_bytes()),
                )
            })
            .transpose()?;
        let normalized_before = change
            .before
            .as_ref()
            .map(|before| {
                run(
                    root,
                    &["hash-object", "--stdin"],
                    None,
                    Some(before.replace("\r\n", "\n").as_bytes()),
                )
            })
            .transpose()?;
        let committed_blob = original.split_whitespace().nth(1);
        ensure!(
            committed_blob == before_blob.as_deref()
                || committed_blob == normalized_before.as_deref()
                || committed_blob == blob.as_deref(),
            "{} has a newer committed version (or a content filter); reconcile the recovery journal manually",
            change.path
        );
        for target in [&snapshot, &live] {
            // Preserve independently staged human changes, including same-path edits.
            if target == &live && staged != original {
                continue;
            }
            if let Some(blob) = &blob {
                run(
                    root,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        mode,
                        blob,
                        &change.path,
                    ],
                    Some(target),
                    None,
                )?;
            } else {
                run(
                    root,
                    &["update-index", "--force-remove", "--", &change.path],
                    Some(target),
                    None,
                )?;
            }
        }
    }
    let tree = run(root, &["write-tree"], Some(&snapshot), None)?;
    if tree != git(root, &["rev-parse", "HEAD^{tree}"])? {
        let commit = run(
            root,
            &["commit-tree", &tree, "-p", &head],
            None,
            Some(format!("ai: {message}\n").as_bytes()),
        )?;
        ensure!(
            git(root, &["symbolic-ref", "HEAD"])? == branch,
            "branch changed during commit; retry recovery"
        );
        git(
            root,
            &[
                "update-ref",
                "-m",
                &format!("ai: {message}"),
                &branch,
                &commit,
                &head,
            ],
        )?;
    }
    // Replace the real index atomically, while retaining independently staged entries.
    let mut replacement = tempfile::NamedTempFile::new_in(index.parent().unwrap())?;
    replacement.write_all(&fs::read(live)?)?;
    replacement.as_file().sync_all()?;
    replacement.persist(index).map_err(|e| e.error)?;
    Ok(())
}

pub fn status(root: &Path) -> Result<Value> {
    if !enabled(root) {
        return Ok(json!({"enabled":false}));
    }
    let upstream = upstream(root).ok();
    let counts = upstream.as_ref().and_then(|_| {
        git(
            root,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .ok()
    });
    let counts: Vec<usize> = counts
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    Ok(
        json!({"enabled":true,"upstream":upstream,"dirty":dirty(root)?,"head":git(root,&["rev-parse","HEAD"]).ok(),"ahead":counts.first(),"behind":counts.get(1),"remote_counts":"relative to last fetched remote refs; status does not contact the remote"}),
    )
}

pub fn sync(root: &Path) -> Result<()> {
    ensure!(enabled(root), "sync requires a Git vault");
    ready(root)?;
    upstream(root)?;
    // Refuse dirty trees rather than stashing, merging, or committing on a read.
    ensure!(
        !dirty(root)?,
        "save/checkpoint local changes before sync (watch --once can checkpoint them)"
    );
    let remote = git(
        root,
        &[
            "config",
            "--get",
            &format!(
                "branch.{}.remote",
                git(root, &["symbolic-ref", "--short", "HEAD"])?
            ),
        ],
    )?;
    git(root, &["fetch", "--", &remote])?;
    let counts = git(
        root,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )?;
    let counts: Vec<usize> = counts
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(counts.len() == 2, "invalid Git ahead/behind response");
    ensure!(
        counts[0] == 0 || counts[1] == 0,
        "local and remote branches diverged; reconcile them manually, then retry sync"
    );
    if counts[1] > 0 {
        git(root, &["merge", "--ff-only", "@{upstream}"])?;
    }
    push(root)
}
