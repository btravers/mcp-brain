# Install and connect clients

## Install a release

Choose your archive from [Releases](https://github.com/btravers/mcp-brain/releases).
Download `SHA256SUMS` alongside it. Compare the archive's SHA-256 hash with the
matching line in that file:

```sh
# macOS
shasum -a 256 mcp-brain-aarch64-apple-darwin.tar.gz

# Linux x86-64 (use aarch64 for ARM64)
sha256sum mcp-brain-x86_64-unknown-linux-musl.tar.gz
```

```powershell
# Windows PowerShell
Get-FileHash .\mcp-brain-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

Extract the archive. On macOS/Linux, install the executable in a directory on
your `PATH`, for example:

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 ./mcp-brain "$HOME/.local/bin/mcp-brain"
mcp-brain --version
```

On Windows, extract `mcp-brain.exe` to a stable directory and either add that
directory to your user `PATH` or use its absolute path in client settings.

The release workflow builds macOS ARM64, Linux x86-64 and ARM64 with musl,
and Windows x86-64 with its compiler runtime linked statically. The executable uses OS facilities; it needs no language
runtime, database, or model credentials. Git-backed vaults also need Git,
working authentication, and a configured upstream.

## Build from source

Install Rust 1.93 or newer with your preferred Rust distribution, then:

```sh
git clone git@github.com:btravers/mcp-brain.git
cd mcp-brain
cargo install --path . --locked
```

The code uses the [official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
and communicates over stdio. No listener or HTTP port is opened.

## Prepare a vault

Pass an existing directory. The recognized folder names are:

```text
0 Inbox/
1 Projects/
2 Areas/
3 Resources/
4 Archive/
5 Meta/
```

Missing folders are created as needed. Root-level Markdown notes can be read
and searched. Hidden paths are excluded. `AGENTS.md` is read through
`vault_context` and cannot be edited with note tools. The server implements
the documented ownership policy; it does not execute arbitrary instructions
found in `AGENTS.md`.

For a Git vault, configure its backup upstream before enabling writes. Verify
the destination yourself, especially for private notes:

```sh
git -C /absolute/path/to/vault remote -v
git -C /absolute/path/to/vault status --short
```

The server uses the vault's existing Git identity and upstream. It never changes
repository visibility, chooses a remote, initializes Git, or force-pushes.
Before a write it commits dirty work as `Save existing vault changes` and pushes.
It commits its own changes separately with an `ai:` prefix and pushes again.
This includes single-note writes that also maintain an index.

Git hooks and signing settings still apply. Authentication and signing must
work without an interactive prompt in the MCP process. A failed initial backup
prevents note changes. See [recovery](inbox.md#recover-from-an-interrupted-write)
for failures after files have been saved.

## Codex

Register the executable:

```sh
codex mcp add brain -- /absolute/path/to/mcp-brain --vault /absolute/path/to/vault serve
codex mcp list
```

Alternatively, add a server table to your user Codex configuration:

```toml
[mcp_servers.brain]
command = "/absolute/path/to/mcp-brain"
args = ["--vault", "/absolute/path/to/vault", "serve"]
```

Use `/mcp` in the Codex terminal UI to inspect the connection. These examples
follow the [official Codex MCP configuration documentation](https://developers.openai.com/codex/mcp).

## Claude Code

Register a user-scoped server so it is available across projects:

```sh
claude mcp add --transport stdio --scope user brain -- /absolute/path/to/mcp-brain --vault /absolute/path/to/vault serve
claude mcp list
```

Use `/mcp` in a new session to inspect it. See the
[Claude Code MCP documentation](https://code.claude.com/docs/en/mcp) for
installation scopes and client approval controls.

## Other stdio MCP clients

Clients accepting `mcpServers` JSON can use:

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

For Windows:

```json
{
  "mcpServers": {
    "brain": {
      "command": "C:\\Tools\\mcp-brain.exe",
      "args": ["--vault", "C:\\Notes\\Vault", "serve"]
    }
  }
}
```

Keep machine paths in user-level configuration, outside public repositories.
When multiple clients use the same vault, they must use the same local state
directory. The default is derived from the canonical vault path. If you set
`--state-dir`, use that same absolute directory in every client and CLI command.

## Make memory useful across sessions

The MCP connection makes memory tools available; it cannot force a model to use
them. Start with a request to consult memory, or put a short preference in each
client's user instructions:

> When relevant, use brain to search existing notes before answering. Capture
> durable insights with remember and consult recall before answering, using the
> relevant global or project scope. Use vault_context to read vault rules.

The preference improves tool discovery. The preservation checks still run in
the server even if a client forgets that preference.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| Client cannot start server | Use absolute executable and vault paths; run `mcp-brain --version` |
| `serve` waits silently | Expected: stdin/stdout carry MCP JSON, not an interactive UI |
| Git write fails | Check upstream, authentication, hooks, and signing in the vault |
| `note changed` | Read again and use the new revision; do not retry stale content |
| Unmarked or non-trailing AI section | Have the human review the section and explicitly mark or relocate it |
| Another operation holds the lock | Wait for it to finish, then retry; all clients share one state directory |
| Unsupported/hidden path | Use a normal `.md` path inside the vault; aliases and hidden components are blocked |
