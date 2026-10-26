# Try mcp-brain with a practice vault

This tutorial uses invented notes in a new folder. You will read a human note,
add a protected AI comment, and capture a memory.

## 1. Create a practice vault

Install mcp-brain using the [setup guide](setup.md). Create a folder called
`brain-practice`, then a child folder called `0 Inbox`. You can use your file
manager or these commands on macOS/Linux:

```sh
mkdir -p "$HOME/brain-practice/0 Inbox"
```

Create `0 Inbox/Garden ideas.md` with this content in a text editor:

```markdown
---
author: human
---
# Garden ideas

I want to learn which herbs can grow in small pots.
```

Keep this practice folder outside a Git repository. It does not need Obsidian
installed or the other PARA folders created in advance.

## 2. Inspect it locally

```sh
mcp-brain --vault "$HOME/brain-practice" status
```

The JSON output reports one Inbox capture. Reminders are initially disabled.
The command creates only local application state outside the vault.

On Windows, replace the path with an absolute Windows path, such as
`"C:\Users\Example\brain-practice"`.

## 3. Register the practice vault

For Codex:

```sh
codex mcp add brain-practice -- mcp-brain --vault "$HOME/brain-practice" serve
```

For Claude Code:

```sh
claude mcp add --transport stdio --scope user brain-practice -- mcp-brain --vault "$HOME/brain-practice" serve
```

If the client cannot find `mcp-brain`, replace it with the executable's absolute
path. Start a new client session and check `/mcp`.

## 4. Comment on the human note

Ask:

> Use brain-practice to read Garden ideas. Add one question to its AI section
> that would help me decide what to learn next.

The assistant should call `read_note`, then `comment_note` with the returned
revision. Open the file again. Its original lines remain intact, followed by a
marked `## AI` section.

Ask the assistant to rewrite the human body. The server rejects an
`update_ai_note` call for this note, regardless of the assistant's instructions.

## 5. Capture an AI memory

Ask:

> Capture a new note called Pot research.md. Record that this is a reference
> topic about growing herbs in small containers, with no deadline or commitment.

The server creates the note under `0 Inbox` with `author: ai`, a creation date,
and an empty trailing `## Notes` section. Because the folder now has two notes,
it also creates `0 Inbox/0 Inbox.md` and adds missing `up` fields to its children.

Write a personal sentence under `## Notes` yourself. Then ask the assistant to
revise the AI note. Your trailing section survives unchanged.

## 6. Review the Inbox

Ask the assistant to review the captures and suggest PARA destinations. The
assistant should classify by actionability, ask when a note fits two places,
and prepare a plan. Use the [Inbox guide](inbox.md) to inspect and apply it.

The human Garden ideas note now has `up: "[[0 Inbox]]"`. To move it, change
that field yourself to the agreed destination's parent, then have the assistant
read the note again and regenerate the proposal. The server refuses to replace
an existing human metadata line.

## 7. Remove the practice connection

```sh
codex mcp remove brain-practice
claude mcp remove brain-practice
```

Run only the command for the client you configured. You can keep the practice
folder as a reference or remove it yourself. mcp-brain has no note deletion tool.
