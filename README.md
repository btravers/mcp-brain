# mcp-brain

Portable LLM memory shared across AI providers, stored as Markdown.

Connect Claude Code, Codex, or another local MCP client to the same Obsidian
vault. Remember preferences, facts, decisions, and procedures; recall them from
another provider; and preserve human notes through ownership checks in Rust.

**Early release:** the ownership and workflow boundaries are tested, but this is
not a filesystem sandbox. Start with the [practice vault tutorial](docs/tutorial.md).

## What it does

- Provides `remember`, `recall`, `update_memory`, and `archive_memory` tools.
- Separates global and project memories, records provenance and review dates,
  and retains previous versions without returning stale history as current memory.
- Lists, searches, and reads Markdown notes, with PARA filters and pagination.
- Creates AI notes in Inbox and maintains folder indexes and parent links.
- Edits AI-authored bodies while retaining human `## Notes` verbatim.
- Appends or revises a marked `## AI` section in human notes.
- Counts Inbox captures and prepares sorting proposals for local approval.
- Offers opt-in review reminders, with due checks and calendar export.
- Commits exact AI snapshots and separate human checkpoints to a configured Git backup.
- Exposes explicit fast-forward sync, freshness, pending pushes, and recovery.
- Optionally watches human saves and supports local commits while offline.
- Reviews overdue memories, changed sources, and possible duplicates or conflicts.
- Builds a private full-text index and bounds recall context by a character budget.
- Creates PARA folders with `init` and checks setup with `doctor`.

One native executable. No Python, Node.js, model API key, or database is needed
at runtime. Git is required only for Git-backed vaults. Indexed search matches
words; literal search remains available. Your connected assistant supplies the
reasoning.

## Install

Download a platform archive and `SHA256SUMS` from
[GitHub Releases](https://github.com/btravers/mcp-brain/releases), verify its
checksum, extract it, and put the executable on your `PATH`.

| Platform | Archive target |
| --- | --- |
| macOS, Apple Silicon | `aarch64-apple-darwin.tar.gz` |
| Linux, x86-64 / amd64 | `x86_64-unknown-linux-musl.tar.gz` |
| Linux, ARM64 / aarch64 | `aarch64-unknown-linux-musl.tar.gz` |
| Windows, x86-64 | `x86_64-pc-windows-msvc.zip` |

Linux releases use musl. Windows ARM64 and Intel macOS binaries are not part of
the initial release matrix. macOS binaries are not notarized.

With Rust 1.93 or newer:

```sh
git clone git@github.com:btravers/mcp-brain.git
cd mcp-brain
cargo install --path . --locked
```

## Connect your assistant

Use your own absolute executable and vault paths:

```sh
codex mcp add brain -- /absolute/path/to/mcp-brain --vault /absolute/path/to/vault serve
claude mcp add --transport stdio --scope user brain -- /absolute/path/to/mcp-brain --vault /absolute/path/to/vault serve
```

For a new vault, run `mcp-brain --vault /absolute/path/to/vault init` first.
Use `doctor` to check setup. See [sync, watching, and offline settings](docs/setup.md#synchronize-and-work-offline)
for optional automation; reads remain local by default.

Both clients launch the same executable against the same files. The server
advertises its tools and instructions through MCP. Each client still needs its
own registration. See [client setup](docs/setup.md) for JSON/TOML configuration,
Windows examples, and how to check the connection.

Ask your assistant:

> Remember that I prefer concise explanations with runnable examples. This is
> a global preference that I am explicitly stating now.

Then, from another connected provider:

> Recall my communication preferences before answering.

These records live in your vault, independently of a provider's built-in memory.
Built-in memories are not imported automatically. Read the
[shared memory guide](docs/memory.md) for scope, updates, and retention.

You can also use ordinary notes as references:

> Use brain to find my existing notes about Rust ownership. Read relevant notes
> before answering, and capture new insights in Inbox.

Or:

> Help me review my Inbox. Propose a destination and reason for each item, then
> prepare a plan for me to inspect locally.

## Ownership in the files

| Note | Allowed changes through MCP |
| --- | --- |
| `author: human`, missing, or unrecognized author | Append/revise the marked trailing AI section; add missing parent metadata during organization |
| `author: ai` | Replace the body; preserve frontmatter and trailing human Notes |
| Malformed or ambiguous frontmatter | Read for diagnosis; refuse edits |

Human notes use this optional trailing section:

```markdown
## AI
<!-- ai-generated, safe to delete -->
Suggestions, questions, or related links from the assistant.
```

An existing `## AI` heading without the marker remains human-owned. Every edit
requires the current revision. There is no delete tool, generic write tool,
ownership-changing tool, reminder-configuration tool, or plan-approval tool.

These protections govern **calls through mcp-brain**. An agent that can write
directly to the vault or run local approval commands can bypass them. For a
stronger boundary, restrict the agent's filesystem and shell access separately.
See [ownership and privacy](docs/design.md).

## Documentation

- [Tutorial: try a practice vault](docs/tutorial.md)
- [How to share durable memory across providers](docs/memory.md)
- [How to install and connect clients](docs/setup.md)
- [How to review Inbox and configure reminders](docs/inbox.md)
- [Tool and CLI reference](docs/reference.md)
- [Ownership, privacy, and design limits](docs/design.md)
- [Contributing and releasing](CONTRIBUTING.md)

## Development

```sh
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Tests use invented notes in temporary directories. They include real stdio MCP
sessions and a local Git backup fixture. No personal vault is needed.

MIT licensed. Author: Benoit TRAVERS.
