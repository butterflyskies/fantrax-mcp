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
- FanGraphs projections endpoint returns 403 Forbidden — scraping blocked (issue #12)

When adding new endpoint support: call the live API first, capture the response, and build your parser and tests against that.

## Smoke/integration tests (MANDATORY)

**Every tool that hits an external API must have a live endpoint smoke test.** This is a coding standard, not a nice-to-have.

- Smoke tests verify: endpoint returns 200, response shape matches parser expectations, key fields are present.
- Run smoke tests in CI or as a pre-merge check. If the endpoint is down or returns unexpected data, the test fails and the PR doesn't merge.
- Pre-existing tools without smoke tests need them added retroactively.
- This rule exists because issue #12 (FanGraphs 403) shipped without any live endpoint test and went undetected until a user tried to use it.

## Building

```bash
cargo build
cargo check
```
