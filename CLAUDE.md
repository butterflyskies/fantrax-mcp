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

## Building

```bash
cargo build
cargo check
```
