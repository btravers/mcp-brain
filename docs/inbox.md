# Review Inbox and configure reminders

## Prepare and approve a sorting plan

Ask your assistant to call `inbox_review`. It returns the capture count,
previews, related folder hints, and questions about actionability. Folder hints
use shared words; they are not semantic classification or decisions.

The assistant should read the complete notes, then propose one destination and
reason per item. Classify by the note's current purpose:

| Category | Decision |
| --- | --- |
| Projects | A goal with an end date and a clear completion condition |
| Areas | An ongoing responsibility |
| Resources | Reference material with no obligation |
| Archive | No longer active or relevant |

If two destinations fit, the assistant should ask. It should never merge two
human notes. Once the batch is agreed, `propose_inbox_plan` creates a local plan.
The tool does not change any vault note.

Inspect the complete proposal locally:

```sh
mcp-brain --vault /absolute/path/to/vault inbox show PLAN_ID
```

The output includes proposed moves, their reasons, and before/after text for
every affected note or index. It contains private note content: keep it local.

Apply it from an interactive terminal:

```sh
mcp-brain --vault /absolute/path/to/vault inbox apply PLAN_ID
```

The command shows the same changes and asks you to type the full plan ID.
There is no `--yes` flag or MCP apply tool. Plans expire after 24 hours and are
rejected if the vault's note snapshot has changed. Generate a fresh proposal
after editing any note.

This is a boundary against an MCP-only caller. An assistant with unrestricted
terminal access could run the local command in a PTY; limit that access if
you require approval to remain exclusively yours.

## What moves can do

- Move Inbox notes to Projects, Areas, Resources, or Archive, keeping filenames.
- Add missing `up` metadata without replacing existing human metadata lines.
- Update `up` on AI notes when their parent changes.
- Create a folder index when a folder gains its second child note.
- Add missing child links to existing indexes in their AI-owned content.
- Repair exact vault-relative wikilinks in AI-owned text, including aliases,
  heading references, and embeds.

A move is blocked if it would collide with another note, create ambiguous short
links, rewrite a human `up` field, or require changing a path link in human text.
Protected `## Notes` links are human text too. The error explains the issue;
the human can repair it and request a new plan.

For example, to move a human note to `3 Resources/Gardening/Herbs.md`, the human
must change an existing `up: "[[0 Inbox]]"` to `up: "[[Gardening]]"` themselves.
The server can add `up` if no such field exists.

Short links such as `[[Herbs]]` survive moves when the filename is unique.
Link rewriting is deliberately limited: relative Markdown links/images
in moved notes and Markdown links to moved notes are blocked. Noncanonical
wikilink paths also need manual repair. Queries in Obsidian plugins, arbitrary
HTML links, canvases, and other non-Markdown formats are outside the resolver.
Use Obsidian's link checks after organizing notes that depend on such features.

The workflow handles Inbox captures; it does not yet rename folders or archive
whole completed projects. Existing indexes are retained even if a folder later
has fewer than two notes. Historical links in them continue to point to moved
notes. Higher-level folder trees are not rebuilt automatically.

## Enable a review reminder

`inbox_status` suggests offering a reminder when the Inbox is nonempty and the
user has not made a choice. The MCP does not enable reminders itself.

For a weekly cadence:

```sh
mcp-brain --vault /absolute/path/to/vault reminder configure --every-days 7
```

This starts the interval from configuration time. After a successful plan is
applied, the review date is updated. To record a review that required no moves:

```sh
mcp-brain --vault /absolute/path/to/vault inbox mark-reviewed
```

Inspect status or check whether a reminder is due:

```sh
mcp-brain --vault /absolute/path/to/vault inbox status
mcp-brain --vault /absolute/path/to/vault reminder check --exit-code
```

`check` prints a short message only when due. With `--exit-code`, it returns
`3` when due and `0` otherwise. Errors return a nonzero error status. A reminder
is due only when the Inbox is nonempty and the interval has elapsed.

During an assistant session, `inbox_status` and `vault_context` expose the due
status. A stdio server is not a background scheduler: it cannot notify you when
the client is closed.

## Get reminders outside an assistant session

Export a calendar reminder after enabling a cadence:

```sh
mcp-brain --vault /absolute/path/to/vault reminder calendar --time 18:00 > inbox-review.ics
```

Import the file in a calendar application that supports recurring iCalendar
events and display alarms. The first event is tomorrow at the chosen local
time; subsequent events repeat at the configured interval. The file contains
no vault paths, note titles, or note contents.

The calendar event is independent of due status. It can remind you even when
the Inbox is empty; marking a review does not reschedule an imported event.
Changing or disabling the CLI reminder does not alter your calendar. Edit or
remove the imported series there, and avoid importing duplicate series.

For a reminder based on actual Inbox state, configure your OS scheduler to run
`reminder check --exit-code` and display a notification when it returns `3`.
Use absolute paths. mcp-brain does not install scheduler jobs or send messages.

## Disable the built-in reminder

```sh
mcp-brain --vault /absolute/path/to/vault reminder disable
```

This records your choice so the assistant is no longer prompted to offer setup.
Remove any imported calendar series or OS scheduler job separately.

## Recover from an interrupted write

Each mutation validates its inputs, creates a local recovery journal, writes
destinations using temporary files, then removes moved sources. Individual
replacements are atomic; the complete multi-file operation is not a filesystem
transaction. The journal contains the originals and intended results.

Under the default strict policy, a failed initial checkpoint or push prevents
note changes. If a later commit or push fails, the write reports a warning.
**Do not repeat the edit.** Inspect the current state:

```sh
mcp-brain --vault /absolute/path/to/vault sync-status
```

A **pending push** means the note changes are already committed locally. Restore
connectivity/authentication, then run:

```sh
mcp-brain --vault /absolute/path/to/vault recover push
```

An **unfinished transaction** means a file operation or commit needs recovery.
Reads remain available; further note writes, watching, and sync are blocked.
Stop other writers, retain a private copy of the journal, and inspect it:

```sh
mcp-brain --vault /absolute/path/to/vault recover show transaction-EXAMPLE.json
mcp-brain --vault /absolute/path/to/vault recover finish transaction-EXAMPLE.json
```

Use the actual journal filename from `sync-status`. `finish` displays every
before/after value and requires typing the filename in an interactive terminal.
It resumes only when each current file exactly matches its recorded before or
after state. Later edits stop recovery before any remaining file is changed.
Newer committed versions also stop automatic commit recovery. Nothing is
rolled back automatically. The command commits the intended snapshots, removes
the completed journal, and attempts a push; a failed upload becomes pending.

If recovery reports later edits, compare them with the journal and reconcile
manually while preserving all human work. After verifying files, Git history,
index, and backup, move the resolved journal outside the active state directory.
A process failure between updating Git HEAD and replacing its index can leave
staged differences; inspect these and preserve any independently staged human
work before declaring recovery complete. Never blindly reset or clean a vault.

An inbox plan interrupted after its writes may remain without an applied-plan
receipt. Recover the journal first, inspect the resulting Inbox, then generate
a new plan instead of applying the old one again. A crash or client timeout can
occur after a successful write: always read the current revision before retrying.
