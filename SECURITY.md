# Reporting a security issue

Do not include real notes, credentials, personal data, or client material in a
public report. Use invented content and relative paths.

For a vulnerability involving loss of human text or a path escape, contact
the maintainer privately at the author email in `Cargo.toml`. Include the
mcp-brain version, operating system, and a minimal fictional reproduction.

The current boundary and limitations are described in
[ownership and privacy](docs/design.md). Direct filesystem access by the agent
and hostile local processes are outside the MCP-only protection model.
