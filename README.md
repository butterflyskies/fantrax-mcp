# fantrax-mcp

Fantasy baseball MCP server for [Fantrax](https://www.fantrax.com/). Exposes roster analysis, lineup optimization, MLB game context, FanGraphs projections, and a multi-agent recommendation ledger via the [Model Context Protocol](https://modelcontextprotocol.io/) (streamable HTTP).

## Features

- Multi-league support with mixed scoring formats (H2H, roto, hybrid, custom)
- Platoon-aware lineup optimization (batter handedness vs. probable starter)
- Morning briefings: yesterday's results, today's action items, recommendation history
- FanGraphs rest-of-season projections (Steamer, ZiPS, ATC, etc.)
- Recommendation ledger for multi-agent debate and outcome tracking
- SQLite-backed caching to minimize external API calls
- Read-only: no roster mutations

## MCP Tools

| Tool | Description |
|------|-------------|
| `list_leagues` | List all fantasy leagues for the configured user |
| `get_standings` | Current standings for a league |
| `get_rosters` | Team rosters for a league and scoring period |
| `get_league_info` | League metadata: teams, matchup schedules, roster constraints, scoring config |
| `get_player_ids` | All player IDs for a sport (defaults to MLB) |
| `get_probable_starters` | Today's MLB probable starting pitchers with handedness |
| `get_player_snippet` | A player's stat line from a specific date's box score |
| `optimize_lineup` | Platoon-based start/sit recommendations for your roster |
| `get_briefing` | Morning briefing: what happened, what to do, hot takes |
| `get_projections` | FanGraphs rest-of-season projections for batters or pitchers |
| `log_recommendation` | Log a recommendation (pickup/drop/start/sit/waiver) to the ledger |
| `query_ledger` | Query the recommendation ledger, optionally filtered by agent |
| `record_outcome` | Record the outcome (win/loss/neutral) of a recommendation |
| `ping` | Health check with server version and league count |

## Configuration

Copy the example config and fill in your values:

```bash
cp config.toml.example config.toml
```

The config requires:

- **`fantrax.user_secret_id`** -- your Fantrax API secret (find under account settings or extract from authenticated requests)
- **`[[leagues]]`** -- one or more league entries with `id`, `name`, `league_type`, `lineup`, and optional `keeper` flag
- **`projections.source`** -- FanGraphs projection system (`steamer`, `zips`, `atc`, `thebat`, `thebatx`)
- **`server.bind_addr`** -- bind address (`127.0.0.1` for local, `0.0.0.0` for containers)
- **`server.port`** and **`server.db_path`** -- port and SQLite database path

See `config.toml.example` for all fields with comments.

## Running

### Build and run locally

```bash
cargo build --release
./target/release/fantrax-mcp --config config.toml
```

CLI options:

```
  -c, --config <PATH>           Path to TOML config file [default: config.toml]
  -p, --port <PORT>             Override server port from config
      --bind-addr <ADDR>        Override bind address (e.g. 0.0.0.0 for containers)
      --db-path <PATH>          Override database path from config
      --max-sessions <N>        Maximum concurrent MCP sessions [default: 10]
      --idle-timeout-secs <N>   Idle session timeout in seconds, 0 to disable [default: 14400]
  -V, --version                 Print version
  -h, --help                    Print help
```

The server starts on `http://<bind_addr>:<port>` with:

- `GET /healthz` -- liveness probe
- `GET /readyz` -- readiness probe (verifies DB access)
- `/mcp` -- MCP streamable HTTP endpoint

### With a container

```bash
docker build -t fantrax-mcp .
docker run -p 3001:3001 -v ./config.toml:/config.toml:ro fantrax-mcp
```

Or with a custom db path mounted:

```bash
docker run -p 3001:3001 \
  -v ./config.toml:/config.toml:ro \
  -v fantrax-data:/data \
  fantrax-mcp --config /config.toml
```

### Environment

Set `RUST_LOG` to control log verbosity:

```bash
RUST_LOG=info ./target/release/fantrax-mcp
RUST_LOG=fantrax_mcp=debug,tower_http=info ./target/release/fantrax-mcp
```

### MCP client configuration

Connect any MCP client to `http://127.0.0.1:3001/mcp` using the streamable HTTP transport. Example Claude Code configuration:

```json
{
  "mcpServers": {
    "fantrax": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:3001/mcp"
    }
  }
}
```

## Architecture

See [`docs/design/`](docs/design/) for full design documents:

- [`problem.md`](docs/design/problem.md) -- problem space and user stories
- [`requirements.md`](docs/design/requirements.md) -- functional and non-functional requirements
- [`architecture.md`](docs/design/architecture.md) -- system architecture, data flow, module breakdown

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
