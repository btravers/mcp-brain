# Tool and CLI reference

## MCP tools

The server exposes fourteen tools over stdio. Successful results include structured
JSON and a textual representation for clients. Operation failures use MCP tool
errors. Invalid arguments may be returned as protocol errors by the SDK.

All argument objects reject unknown fields. Paths use `/` separators and are
relative to the configured vault. Pagination offsets are zero-based.

| Tool | Arguments | Result / behavior |
| --- | --- | --- |
| `remember` | `title`, `content`, `memory_type`, `source`, `basis`; optional `scope`, `review_after` | Creates a durable memory note in Inbox; returns stable ID, revision, and write outcome |
| `recall` | Optional `query`, `scope`, `memory_type`, `include_archived`, `offset`, `limit` | Ranked current memories with provenance, review status, annotations, and warnings |
| `update_memory` | `id`, `expected_revision`, `content`, `source`, `basis`, `reason`; optional `review_after`, `reactivate` | Updates current content, retains prior versions and human Notes |
| `archive_memory` | `id`, `expected_revision`, `reason` | Excludes the record from normal recall without deleting or moving it |
| `vault_context` | None | PARA folder counts, `AGENTS.md` content if present, ownership/privacy guidance, Inbox status |
| `list_notes` | Optional `category`, `query`, `offset`, `limit` | Metadata, revisions, total, next offset; literal case-insensitive path/content search |
| `read_note` | `path` | Content, revision, author, category, parse error if any |
| `create_note` | `filename`, `title`, `body` | Creates an AI note in Inbox; returns changed paths and new revisions |
| `update_ai_note` | `path`, `expected_revision`, `content` | Replaces ordinary AI body; memory records require `update_memory` |
| `comment_note` | `path`, `expected_revision`, `content` | Appends to a human note's marked trailing AI section |
| `set_ai_section` | `path`, `expected_revision`, `content` | Replaces only that AI section's content |
| `inbox_status` | None | Capture count, oldest modification time, reminder configuration and due status |
| `inbox_review` | Optional `offset`, `limit` | Previews, lexical folder matches, review questions, status |
| `propose_inbox_plan` | `moves` | Saves a local plan; returns ID, move descriptions, changed paths, expiry, approval command |

`list_notes.limit` defaults to 50 and accepts 1–200. `inbox_review.limit`
defaults to 20 and accepts 1–50. Review previews contain at most 1,200 characters;
read the complete note before classifying it.

Categories are `inbox`, `projects`, `areas`, `resources`, `archive`, and `meta`.
Root notes have a null category. PARA folder names are fixed in this version.
Folder indexes appear in normal listings and category counts; Inbox capture
counts exclude files named after their containing folder.

### Revisions

`revision` is the lowercase hexadecimal SHA-256 of the full UTF-8 note content.
It includes frontmatter, whitespace, line endings, and protected sections.
Pass it back exactly as `expected_revision`. If any content changed, read again
and reconsider the edit. A revision is a concurrency check, not authorization.

### Durable memory

Memory types are `preference`, `fact`, `decision`, and `procedure`. `basis` is
required and is one of `user_stated`, `observed`, or `inferred`; it is a statement
by the caller, not a server-verified confidence score. `source` is a nonempty
description or reference. Never invent a source.

`scope` is `global` by default, or `project:<stable-name>`. Project names may
contain letters, digits, dots, underscores, and hyphens, up to 120 characters.
Use a provider-independent name, not a machine path. Recall for a project
includes global memories as well. `scope: "*"` explicitly searches all scopes;
it is accepted only by `recall`. Scoping selects relevance, not permissions.

`recall.limit` defaults to 20 and accepts 1–100. Ranking counts shared words
between the query and the current title/content. History is excluded. An empty
query lists memories in scope. The response wraps each record as
`{"score": 2, "memory": {...}}` and returns `total`, `next_offset`, and `warnings`.
Malformed records are reported in warnings; duplicate IDs stop retrieval rather
than silently selecting a record.

`review_after` is an optional UTC `YYYY-MM-DD` date. Recall flags `review_due`
on and after that date. Due memories remain visible so the assistant can ask
for verification; they do not become automatically false or archived.

`update_memory` retains an omitted review date and preserves type, scope, title,
and ID. It appends the old content/provenance and the correction reason to
`## History`. Archived records stay archived unless `reactivate: true` is
explicitly supplied. A record physically inside `4 Archive` is excluded from
normal recall regardless of metadata and must be moved out by the human before
reactivation.

`archive_memory` is logical retirement. It records the reason in metadata and
history, retaining the note and its versions. It does not apply a PARA move or
erase data from Git history, client transcripts, or backups. The
[memory guide](memory.md) explains the workflow and portable format.

### New notes

`filename` is one portable `.md` name, without directories. It must not collide
case-insensitively with an existing note filename. `title` is one nonempty line.
`body` omits frontmatter and the reserved `## Notes` section. The server adds:

```yaml
---
created: YYYY-MM-DD
author: ai
tags: []
---
```

`created` uses the host's local date. Parent metadata is added when needed for
folder indexes. Writes return `changed`, a map of `revisions`, and an optional
`warning` if files were saved but Git backup could not finish.

### AI sections and Notes

An editable AI section is a real top-level `## AI` heading immediately followed
by `<!-- ai-generated, safe to delete -->`. It must be the final level 1/2
section. Content may use headings at level 3 or deeper. The server owns the
marker; callers must not provide another one.

AI note updates accept the full replacement body, including its title if
desired. They do not accept frontmatter or `## Notes`. The existing Notes suffix
is copied verbatim, and the resulting Markdown is checked to ensure the suffix
remains a real trailing section. Multiple or non-trailing Notes sections cause
the edit to fail.

### Sorting plans

Each entry in `moves` has these required string fields:

```json
{
  "source": "0 Inbox/Herbs.md",
  "destination": "3 Resources/Gardening/Herbs.md",
  "expected_revision": "<revision from read_note>",
  "reason": "Reference topic without an active goal or ongoing responsibility"
}
```

A plan contains 1–100 moves. Sources must be Inbox notes, not indexes.
Destinations must be Projects, Areas, Resources, or Archive. Filenames remain
unchanged. Plans contain private before/after text in local state and expire
after 24 hours. See [Inbox review](inbox.md) for approval and link restrictions.

## CLI

```text
mcp-brain --vault PATH [--state-dir PATH] COMMAND

serve
status
inbox status
inbox review [--offset N] [--limit N]
inbox show PLAN_ID
inbox apply PLAN_ID
inbox mark-reviewed
reminder configure --every-days N
reminder disable
reminder check [--exit-code]
reminder calendar [--time HH:MM]
```

`--vault` is required before the command. The path must exist. `--state-dir`
is optional and must be outside the vault. `--help` and `--version` work without
a vault. `inbox apply` requires interactive stdin and confirmation of the plan ID.

Most commands print JSON. `serve` reserves stdout for MCP, `reminder check`
prints a human message only when due, and `reminder calendar` prints iCalendar
with CRLF line endings. Operational errors go to stderr and return exit code 1;
argument errors use Clap's exit status. `reminder check --exit-code` returns 3
when due and 0 otherwise.

Reminder intervals accept 1–365 days. Calendar time defaults to `18:00` and uses
floating local time. No reminder is enabled by default.

## Local state

Default state is under the OS's per-user local application-data directory,
inside `mcp-brain/<vault-id>`. The ID is derived from the canonical vault path.
Use `--state-dir` when you need an explicit location. Unix state directories
are mode `0700`; other platforms use the user's normal local permissions.

State contains an advisory lock, reminder preferences, plan files, applied-plan
receipts, and unfinished transaction journals. Plans and journals contain note
contents. Keep state private; do not commit it or place it in a shared folder.
Completed transaction journals are removed. Plans and receipts remain until
you remove them yourself. Moving a vault changes its default state ID.

## Limits

- UTF-8 `.md` files, at most 2 MiB each.
- Hidden paths, traversal, symlinks, Windows reparse points, and Unix hard-linked
  files are rejected by note path operations.
- Portable names only: no Windows reserved names or path punctuation that
  conflicts with Obsidian wikilinks.
- Invalid or unreadable files can make a scan fail rather than silently omit
  content. Unsafe paths are omitted from listings.
- No embeddings, external model calls, network listener, attachments, arbitrary
  file writes, deletes, or arbitrary shell tools.
- Git commands may contact the vault's configured backup; all other note
  operations are local.
