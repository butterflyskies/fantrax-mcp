# fantrax-mcp

Rust MCP server for fantasy baseball (Fantrax). Provides roster analysis, lineup optimization, and multi-agent recommendations via the Model Context Protocol.

## Architecture

See `docs/design/` for full design docs:
- `problem.md` — problem space and user stories
- `requirements.md` — functional and non-functional requirements
- `architecture.md` — system architecture, data flow, module breakdown

## Structure

- `src/main.rs` — CLI entry point, config loading, server startup
- `src/lib.rs` — crate root
- `src/config.rs` — TOML configuration parsing
- `src/fantrax/` — Fantrax HTTP client

## Configuration

Copy `config.toml.example` (or create `config.toml`) with your Fantrax credentials. The file is gitignored.

## Testing philosophy

**Do not trust the Fantrax API documentation.** The beta API's actual wire format diverges from its docs — response shapes, key names, and data presence have all surprised us. All test fixtures must be captured from live API responses, not hand-crafted from documentation. If a test passes against a fixture you wrote by reading the docs, it proves nothing.

Concrete examples of doc/wire divergence:
- Roster endpoint returns an object keyed by team ID, not an array (PR #9)
- Player data lives under `rosterItems`, not `players` or `roster` (PR #9)
- Roster endpoint does not include player names — only IDs (PR #10)

When adding new endpoint support: call the live API first, capture the response, and build your parser and tests against that.

## Building

```bash
cargo build
cargo check
```
