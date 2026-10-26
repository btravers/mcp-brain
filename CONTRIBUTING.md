# Contributing

Use Rust 1.93 or newer. Clone with SSH:

```sh
git clone git@github.com:btravers/mcp-brain.git
cd mcp-brain
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

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

## Code map

| File | Responsibility |
| --- | --- |
| `src/note.rs` | Ownership, frontmatter, Markdown sections, revisions |
| `src/memory.rs` | Portable memory records, scope, provenance, history, recall, retirement |
| `src/vault.rs` | Paths, reads/search, locking, writes, indexes, recovery journals |
| `src/review.rs` | Inbox hints, proposed moves, reminder state |
| `src/git.rs` | Exact AI snapshots, human checkpoints, fast-forward sync, bounded Git subprocesses |
| `src/operations.rs` | Local policy, coordinator workflows, watching, recovery, setup diagnostics |
| `src/index.rs` | Rebuildable word postings and content-hash invalidation |
| `src/server.rs` | Typed MCP tools and initialization instructions |
| `src/main.rs` | Local CLI and human approval boundary |

Avoid exposing a generic write, delete, shell, author-change, or approval tool.
Do not solve a link-repair problem by silently rewriting protected human text.
Keep the documented limitations accurate when extending supported workflows.

## CI and releases

CI runs formatting, tests, and Clippy on macOS ARM64, Linux x86-64, Linux ARM64,
and Windows x86-64. Release builds additionally test the target binaries and
package Linux with musl and Windows with a statically linked compiler runtime.
See GitHub's [runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
for the platform labels used in the workflows.

To prepare a release:

1. Update the package version and lockfile; run the checks above.
2. Review tracked files for private information and update the documentation.
3. Push a matching `vX.Y.Z` tag.
4. Wait for the Release workflow to build all four archives and `SHA256SUMS`.
5. Inspect the generated **draft** GitHub release, then publish it manually.

The release workflow does not publish drafts automatically. A manual workflow
run builds downloadable Actions artifacts without creating a release. There is
no crates.io publication step, signing, or macOS notarization in this version.
