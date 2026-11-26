# Contributing

`rust-toolchain.toml` pins the Rust version. The Nix flake provides that
exact toolchain, and CI uses the same shell on Linux and macOS:

```sh
git clone git@github.com:btravers/mcp-brain.git
cd mcp-brain
nix develop
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The shell also provides `rust-analyzer` and `rust-src` for editors and
`shellcheck` for `install.sh`. With direnv, `direnv allow` loads it on `cd`.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same version.

## Changes worth testing

Ownership checks are the core contract. Include a regression test for changes
that affect frontmatter, section parsing, path handling, stale writes, locking,
sorting plans, Git checkpoints, or recovery. Use byte-for-byte assertions for
human text and protected sections. Protocol changes should exercise a real
stdio client, not only direct library calls.

Tests must use temporary directories and invented data. Never run mutation
tests against a contributor's actual vault. Keep personal paths, private notes,
tokens, client identifiers, and transcripts out of fixtures and issue reports.
The author identity in package metadata is intentionally public.

Semantic tests that need the real model are ignored by default and do not run
in CI. After changing chunking, pooling, or similarity floors, download the
pinned files (see `src/semantic.rs`) and run:

```sh
MCP_BRAIN_TEST_MODEL_DIR=/path/to/files cargo test --release --test semantic -- --ignored --nocapture
```

The printed similarities are the evidence for the floors; update the comments
next to them when they move.

## Code map

| File | Responsibility |
| --- | --- |
| `src/note.rs` | Ownership, frontmatter, Markdown sections, revisions |
| `src/memory.rs` | Portable memory records, scope, provenance, history, recall, retirement |
| `src/vault.rs` | Paths, reads/search, locking, writes, indexes, recovery journals |
| `src/review.rs` | Inbox hints, proposed moves, reminder state |
| `src/git.rs` | Exact AI snapshots, human checkpoints, fast-forward sync, bounded Git subprocesses |
| `src/operations.rs` | Local policy, coordinator workflows, watching, recovery, setup diagnostics |
| `src/index.rs` | Rebuildable stem postings, BM25, and content-hash invalidation |
| `src/semantic.rs` | Optional local embeddings: model install, sections, vector cache, fusion |
| `src/server.rs` | Typed MCP tools and initialization instructions |
| `src/main.rs` | Local CLI and human approval boundary |
| `src/terminal.rs` | Human-readable views and resumable, line-oriented review |
| `src/screen.rs` | Ratatui full-screen review, Markdown, input, scrollbars and immediate item application |
| `src/picker.rs`, `src/plans.rs` | Recent-plan selection, filtering, progress and freshness |
| `src/undo.rs` | Guarded reversal of individual saves, receipts and generated-index archives |

Avoid exposing a generic write, delete, shell, author-change, or approval tool.
Do not solve a link-repair problem by silently rewriting protected human text.
Keep the documented limitations accurate when extending supported workflows.

## CI and releases

CI runs formatting, tests, and Clippy on macOS ARM64, Linux x86-64, Linux ARM64,
and Windows x86-64: inside `nix develop` on Linux and macOS, and with rustup on
Windows, where Nix does not run. It also runs `install.sh` against the latest
published release on Linux x86-64, Linux ARM64, and macOS, and checks it with
shellcheck. Release builds additionally test the target binaries and
package Linux with musl and Windows with a statically linked compiler runtime.
See GitHub's [runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
for the platform labels used in the workflows.

To prepare a release:

1. In a PR, update the package version and lockfile; run the checks above.
2. Review tracked files for private information and update the documentation.
3. Merge the PR. The Release workflow sees an untagged version in `Cargo.toml`,
   builds all four archives and `SHA256SUMS`, creates the `vX.Y.Z` tag on the
   merge commit, and opens a **draft** release.
4. Inspect the draft GitHub release, then publish it manually.

Pushes to `main` whose version is already tagged build nothing. If a release
build fails, no tag is created; re-run the workflow after fixing the cause. If
the tag exists but the draft is missing, delete the tag and re-run.
The release workflow does not publish drafts automatically. A manual workflow
run builds downloadable Actions artifacts without creating a release. There is
no crates.io publication step, signing, or macOS notarization in this version.

Dependabot opens weekly PRs for Cargo dependencies, the Rust toolchain, flake
inputs, and GitHub Actions. When it bumps `rust-toolchain.toml`, also raise
`rust-version` in `Cargo.toml` if the code starts relying on the new release.
