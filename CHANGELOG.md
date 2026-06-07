# Changelog

## v0.1.1

### Bug Fixes

- Use `NoArgs` struct for parameterless tools (`list_leagues`, `ping`) — `Parameters<()>` generated `type: "null"` schemas that violate MCP's `type: "object"` requirement, causing Claude Code to reject the entire tool list
- Added invariant test: all tool arg structs must produce object-type schemas

## v0.1.0

Initial release.

### Features

- 14 MCP tools: league listing, standings, rosters, league info, player IDs, probable starters with handedness, player game lines from box scores, platoon-based lineup optimization, morning briefings, FanGraphs projections, recommendation ledger (log/query/record outcome), and health check
- Fantrax, MLB Stats API, and FanGraphs API clients
- SQLite caching with TTL and probabilistic eviction
- Platoon analysis engine for start/sit recommendations
- Multi-league support with configurable league type, lineup type, and keeper settings
- Streamable HTTP MCP transport with bounded session management
- CLI with `--port`, `--bind-addr`, `--db-path` overrides
- `/healthz` and `/readyz` endpoints
- Secret handling via `secrecy` crate with redacted Debug output

### Bug Fixes

- Switch to `parking_lot::Mutex` for cancel-safe, non-poisoning DB locks
- All DB calls from async handlers use `_async` wrappers via `spawn_blocking`
- `LogRecommendationArgs.players` typed as `Vec<PlayerId>` instead of raw `String`
- `PlayerType` variants collapsed with `#[serde(alias)]`
- Dead `validate_response` function removed
- Cache boilerplate extracted to `cached_fetch` generic helper
