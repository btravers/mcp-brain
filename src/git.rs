//! Git is optional for plain folders. In a Git vault, preserve and push a human
//! checkpoint before any write, then commit and push the AI changes separately.
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("could not run git")?;
    ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim_end().into())
}

pub fn enabled(root: &Path) -> bool {
    root.join(".git").exists()
}

pub fn checkpoint(root: &Path) -> Result<()> {
    if !enabled(root) {
        return Ok(());
    }
    // A configured upstream is required before any edits: do not guess where
    // private notes should go, or silently leave the expected backup disabled.
    git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .context("Git vault needs a configured backup upstream before MCP writes")?;
    if !git(root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        git(root, &["add", "--all"])?;
        git(root, &["commit", "-m", "Save existing vault changes"])?;
    }
    git(root, &["push"])?;
    Ok(())
}

pub fn finish(root: &Path, paths: &[String], message: &str) -> Result<()> {
    if !enabled(root) {
        return Ok(());
    }
    let mut args = vec!["add", "--all", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(root, &args)?;
    if !git(root, &["diff", "--cached", "--name-only"])?.is_empty() {
        // Commit only our paths. A concurrently staged human file is not ours.
        let msg = format!("ai: {message}");
        let mut args = vec!["commit", "--only", "-m", &msg, "--"];
        args.extend(paths.iter().map(String::as_str));
        git(root, &args)?;
    }
    git(root, &["push"])?;
    Ok(())
}
