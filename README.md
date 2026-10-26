# mcp-brain

[![CI](https://github.com/btravers/mcp-brain/actions/workflows/ci.yml/badge.svg)](https://github.com/btravers/mcp-brain/actions/workflows/ci.yml)

**One Markdown memory for your AI assistants.**

Remember a preference in Claude Code. Recall it in Codex. Keep the record in
your own Obsidian vault, alongside your notes, with human text protected by
the MCP server.

mcp-brain is a native Rust executable for local Markdown and Obsidian PARA
vaults. No Python, Node.js, model API key, or database service is required.
Git is optional; Obsidian is optional too.

[Get started](#get-started) · [Claude Code](#claude-code) · [Codex](#codex) ·
[Other MCP clients](#other-mcp-clients) · [Documentation](#documentation)

## Get started

### 1. Install the executable

Download the archive for your machine and
[SHA256SUMS](https://github.com/btravers/mcp-brain/releases/latest/download/SHA256SUMS)
from the same release. [Verify its checksum](docs/setup.md#install-a-release),
extract it, and put `mcp-brain` on your `PATH` or use its absolute path.

| Platform | Download |
| --- | --- |
| macOS, Apple Silicon | [macOS ARM64](https://github.com/btravers/mcp-brain/releases/latest/download/mcp-brain-aarch64-apple-darwin.tar.gz) |
| Linux, x86-64 / amd64 | [Linux amd64](https://github.com/btravers/mcp-brain/releases/latest/download/mcp-brain-x86_64-unknown-linux-musl.tar.gz) |
| Linux, ARM64 / aarch64 | [Linux ARM64](https://github.com/btravers/mcp-brain/releases/latest/download/mcp-brain-aarch64-unknown-linux-musl.tar.gz) |
| Windows, x86-64 | [Windows amd64](https://github.com/btravers/mcp-brain/releases/latest/download/mcp-brain-x86_64-pc-windows-msvc.zip) |

```sh
mcp-brain --version
```

Linux binaries use musl; Windows binaries include their compiler runtime.
macOS binaries are not notarized. Intel macOS and Windows ARM64 builds are not
currently provided. Downloads live under [Releases](https://github.com/btravers/mcp-brain/releases).

<details>
<summary>Build from source with Rust 1.93 or newer</summary>

```sh
git clone git@github.com:btravers/mcp-brain.git
cd mcp-brain
cargo install --path . --locked
```

</details>

### 2. Choose your vault

Use an existing vault, or create a new one with the PARA directories:

```sh
mcp-brain --vault "/absolute/path/to/vault" init
mcp-brain --vault "/absolute/path/to/vault" doctor
```

`init` preserves existing notes and creates missing directories. It also prints
client configuration examples; it does not register the server for you.
For a Git vault, configure its intended backup upstream and authentication
before writing through MCP. See [vault setup](docs/setup.md#prepare-a-vault).

New to the project? The [practice vault tutorial](docs/tutorial.md) uses invented
notes so you can try the ownership and review workflows first.

## Connect your harness

Choose your assistant below. Replace the example executable and vault paths
with your own **absolute paths**. Register each harness against the same vault.

### Claude Code

Register mcp-brain for your user account, so it is available across projects:

```sh
claude mcp add --transport stdio --scope user brain -- "/absolute/path/to/mcp-brain" --vault "/absolute/path/to/vault" serve
claude mcp list
```

Start a new Claude Code session and use `/mcp` to check the connection.
Then ask:

> Use brain to remember that I prefer concise explanations with runnable
> examples. This is a global preference I am explicitly stating now.

[Claude Code setup details](docs/setup.md#claude-code)

### Codex

Register the same executable and vault:

```sh
codex mcp add brain -- "/absolute/path/to/mcp-brain" --vault "/absolute/path/to/vault" serve
codex mcp list
```

Start a new Codex session and use `/mcp` in the terminal UI to check the connection.
Then ask:

> Use brain to recall my communication preferences before answering.

If you captured the preference from Claude Code above, Codex can retrieve that
same Markdown record. You can also capture memories directly from Codex.

<details>
<summary>Configure Codex with TOML instead of the CLI</summary>

Add this table to your user Codex configuration:

```toml
[mcp_servers.brain]
command = "/absolute/path/to/mcp-brain"
args = ["--vault", "/absolute/path/to/vault", "serve"]
```

</details>

[Codex setup details](docs/setup.md#codex)

### Other MCP clients

Use a client that can launch a local **stdio MCP server**. Clients accepting
`mcpServers` JSON can use:

```json
{
  "mcpServers": {
    "brain": {
      "command": "/absolute/path/to/mcp-brain",
      "args": ["--vault", "/absolute/path/to/vault", "serve"]
    }
  }
}
```

The configuration file location and root key depend on the client. mcp-brain
does not expose an HTTP endpoint. On Windows, use the full path to
`mcp-brain.exe`; see the [Windows JSON example](docs/setup.md#other-stdio-mcp-clients).

All clients must share the same local state directory as well as the vault.
The default is derived from the vault path. If you set `--state-dir`, use the
same value in every client and CLI command. Keep machine paths in local client
settings, outside public repositories.

## What you can do

| Workflow | Tools and behavior |
| --- | --- |
| Share durable memory | `remember`, `recall`, `update_memory`, `archive_memory`; global/project scopes, provenance, and version history |
| Find existing knowledge | `search_notes`, `list_notes`, `read_note`; indexed word search, literal search, PARA filters, and bounded recall |
| Keep memory current | `memory_review` flags overdue records, changed sources, and possible overlaps or conflicts for review |
| Capture and enrich notes | New AI notes land in Inbox; human notes accept suggestions in a marked AI section |
| Consolidate your Inbox | Counts, review hints, and proposed moves with reasons; you inspect and approve the exact changes locally |
| Back up and synchronize | Explicit fast-forward Git sync, separate human/AI commits, pending uploads, and recovery; optional watching and offline writes |

Try these requests with your connected assistant:

> Find my notes about Rust ownership. Read the relevant notes before answering.

> Help me review my Inbox. Propose a destination and reason for each item, then
> prepare a plan for me to inspect locally.

> Review my memories for outdated information and changed sources. Show me the
> findings before proposing corrections.

Local commands default to readable text and tables; add `--json` for scripts
and LLMs. Use `inbox review` to pick a recent plan by note names and progress,
or `inbox review PLAN_ID` to open it directly in a full-screen queue with formatted
Markdown, scrolling and visible actions. Apply a move immediately, drop its
proposal, or leave precision for the assistant. Saves advance to the next item,
and you can resume the same review. Press `u` to undo the last saved move;
later edits block reversal. `--plain` keeps the basic batch workflow.
See the [Inbox review guide](docs/inbox.md#review-one-item-at-a-time).

Memory lives in ordinary Markdown, independently of a provider's built-in
memory. Existing provider memories are not imported automatically. The harness
must call `recall` for shared memory to influence its answer. See the
[shared memory guide](docs/memory.md) for the complete workflow.

## How your notes are protected

Ownership checks run in Rust on every edit. They do not depend on the assistant
remembering a rule in its instructions.

| Note | Allowed changes through MCP |
| --- | --- |
| `author: human`, missing, or unrecognized author | Append/revise the marked trailing AI section; add missing parent metadata during organization |
| `author: ai` | Edit AI content while preserving the trailing human `## Notes` section; dedicated memory tools retain history |
| Malformed or ambiguous frontmatter | Refuse edits; allow reading for diagnosis |

Suggestions in a human note use this trailing section:

```markdown
## AI
<!-- ai-generated, safe to delete -->
Suggestions, questions, or related links from the assistant.
```

An unmarked `## AI` heading remains human-owned. Every edit requires the current
revision. Inbox moves need local approval, and there is no delete tool.

**These protections apply to calls through mcp-brain.** An assistant with direct
filesystem or unrestricted shell access can bypass them. Restrict that access
separately when you need a stronger boundary. Note contents returned to a client
may be sent to its model provider. See [ownership and privacy](docs/design.md).

## Optional Git automation and reminders

Reads stay local by default. For a Git vault, strict backup requires a successful
initial push before an MCP note write. AI commits contain the planned text and
bypass commit hooks and automatic signing; human checkpoints use ordinary Git
commit. Review the [Git setup and limitations](docs/setup.md#prepare-a-vault).

| I want to… | Start here |
| --- | --- |
| Check freshness, sync, or enable offline writes | [Sync and local configuration](docs/setup.md#synchronize-and-work-offline) |
| Back up human saves after editing stops | [Foreground watcher](docs/setup.md#back-up-human-saves-from-obsidian) |
| Retry a failed push or interrupted write | [Recovery](docs/inbox.md#recover-from-an-interrupted-write) |
| Set a review cadence and get calendar reminders | [Inbox and reminders](docs/inbox.md) |

Watching, background sync, offline writes, and reminders are opt-in. No daemon,
calendar event, or OS scheduler job is installed automatically.

## Documentation

- [Practice vault tutorial](docs/tutorial.md) — try capture, ownership, and review.
- [Shared memory guide](docs/memory.md) — scopes, provenance, corrections, and retention.
- [Setup guide](docs/setup.md) — installation, harness configuration, Git, and troubleshooting.
- [Inbox guide](docs/inbox.md) — approve sorting plans and configure reminders.
- [Tool and CLI reference](docs/reference.md) — MCP tools and local commands.
- [Design and privacy](docs/design.md) — guarantees, tradeoffs, and limitations.

## Contribute or get help

Use [Discussions](https://github.com/btravers/mcp-brain/discussions) for questions
and ideas, and [Issues](https://github.com/btravers/mcp-brain/issues) for bugs.
Use invented examples in public reports; keep private notes and credentials out.
See [CONTRIBUTING.md](CONTRIBUTING.md) for development and release instructions,
or [SECURITY.md](SECURITY.md) for security reporting.

Tests use temporary vaults and invented notes, including real stdio MCP sessions
and local Git remotes. CI covers macOS ARM64, Linux amd64/ARM64, and Windows amd64.

[MIT licensed](LICENSE). Author: Benoit TRAVERS.
