# Share durable memory across providers

Use mcp-brain as the shared source for long-lived context. A preference captured
from Claude Code can be recalled from Codex because both clients read the same
Markdown record. The files also work after a private vault sync to another
machine. No provider account or proprietary memory format is embedded in them.

## Choose memory or reference

| Use | Tool |
| --- | --- |
| A durable communication preference | `remember`, type `preference` |
| An explicit fact relevant to future work | `remember`, type `fact` |
| A project decision and its rationale | `remember`, type `decision`, project scope |
| A repeatable procedure | `remember`, type `procedure` |
| Draft thoughts, research, a question, or general reference | Ordinary note tools |

Human notes remain references. The server does not mine them automatically for
claims about you. Ask the assistant to extract a specific memory when you want
that transformation, and preserve its source and uncertainty.

## Capture a memory

Ask a connected assistant:

> Remember that I prefer concise answers with runnable examples. This is a
> global preference I am explicitly stating now.

A corresponding tool call is:

```json
{
  "title": "Communication preferences",
  "content": "Prefer concise explanations with runnable examples.",
  "memory_type": "preference",
  "scope": "global",
  "source": "The user explicitly stated this preference in the current conversation.",
  "basis": "user_stated"
}
```

The `remember` tool creates an AI-authored note in `0 Inbox`, returning its
stable UUID and current revision. An exact duplicate active statement in the
same scope is rejected. The assistant should recall first, because differently
worded duplicates and contradictions require judgment.

Use `observed` for a conclusion based on observed work, and `inferred` for a
tentative interpretation. These labels are supplied by the assistant or caller;
the server does not independently verify them. Avoid retaining secrets or
unauthorized client material, even if a source is provided.

## Recall from another provider

Connect another MCP client to the same vault and ask:

> Recall my communication preferences before explaining this topic.

The client can call `recall` with `{"query":"communication preferences"}`.
Omitting scope searches global memories only. To work on a particular project:

```json
{"query":"testing conventions", "scope":"project:example-app"}
```

This returns that project's memories plus global ones. Other projects are not
included by default. Use `{"scope":"*"}` only when you intentionally want to
search across projects. Scopes control relevance, not permission.

Recall ranks lexical matches in the current title and statement. It returns
source, basis, review status, and revision alongside the statement. Old versions
stay in the note but are excluded from recall. Human annotations under `## Notes`
are returned separately so they can qualify a statement without being silently
converted into asserted memory.

For a time-sensitive memory, supply `review_after`, such as `2027-01-15`.
Recall flags it as due on and after that UTC date. The assistant should verify
it before relying on it. This is separate from an Inbox review reminder and
does not schedule a notification.

## Correct a memory

Ask:

> Update my communication preference: include a brief rationale before the
> runnable example. This replaces the earlier preference.

The assistant recalls the record and calls `update_memory` with its ID, current
revision, replacement statement, source, basis, and reason. The server stores
the old statement and provenance under `## History`, then replaces only the
current statement. Human `## Notes` stays byte-for-byte intact.

Generic `update_ai_note` calls are refused for memory records, so an assistant
cannot accidentally erase their history through the ordinary note interface.
Type, scope, title, and ID are stable through memory updates. If those must
change, capture a replacement in the correct scope and archive the old record.

## Stop recalling an outdated memory

Ask the assistant to call `archive_memory` with its ID, revision, and reason.
The record is marked archived, with a history entry explaining why. Default
recall excludes it. You can inspect retired records with `include_archived: true`.

Archiving is not deletion: the Markdown note, previous versions, and Git history
remain. It also does not move the file into PARA Archive; file organization
continues through the separate Inbox review workflow. A record placed in
`4 Archive` is also excluded from normal recall regardless of its active flag.

To restore a logically archived record, use `update_memory` with
`reactivate: true`. If its file is physically inside `4 Archive`, move it out
yourself first. None of these operations erases provider transcripts or backups.

## Inspect the portable file

Memory files use this shape; the ID and dates below are illustrative:

```markdown
---
created: 2026-01-01
author: ai
tags: []
kind: memory
memory_id: 00000000-0000-4000-8000-000000000001
memory_title: Communication preferences
memory_type: preference
scope: global
source: User explicitly stated the preference.
basis: user_stated
memory_status: active
updated: '2026-01-01T12:00:00Z'
---
# Communication preferences

## Memory

Prefer concise explanations with runnable examples.

## History

## Notes
```

The filename contains the ID and may move through PARA without changing that ID.
Folder indexes and `up` metadata work like other AI notes. If two files have the
same memory ID, recall stops and asks for repair rather than choosing one.
Malformed records are reported as warnings and excluded from memory results.

Markdown is the portable storage layer; the dedicated tools supply the memory
semantics. No special database needs to accompany the files between machines.
Concurrent clients on one machine must still share the same state directory for
locking. Keep the vault private when syncing it.

## Built-in provider memory

mcp-brain does not automatically import Claude's, Codex's, or another provider's
built-in memory. If a provider offers an export, review it yourself and explicitly
ask for appropriate statements to be captured with `remember`. Avoid importing
entire transcripts or treating an old assistant summary as a verified fact.

Register the server in each client and ask it to use `recall` before answering
when relevant. MCP makes a common memory available; the client still decides
when to call it. The [setup guide](setup.md) includes a short instruction you can
reuse in client preferences.
