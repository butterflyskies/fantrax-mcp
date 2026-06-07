<!-- design-meta
status: draft
last-updated: 2026-05-26
phase: 2
-->

# Requirements: Fantrax Fantasy Baseball MCP Server

## Use Cases

| ID | Actor | Use Case | Priority |
|----|-------|----------|----------|
| UC-01 | Agent | View current roster with player stats, injury status, and player snippets | Must |
| UC-02 | Agent | View league standings and category breakdowns | Must |
| UC-03 | Agent | View current matchup and opponent's roster | Must |
| UC-04 | Agent | Get player projections for the week | Must |
| UC-05 | Agent | Suggest waiver wire pickups based on roster gaps | Must |
| UC-06 | Agent | Suggest optimal lineup for upcoming games (platoon-aware) | Must |
| UC-07 | Agent | Compare two agents' independent recommendations via ledger | Must |
| UC-08 | User | Configure credentials; auto-discover leagues via getLeagues | Must |
| UC-09 | Agent | Generate morning briefing (results, actions, hot takes) | Must |
| UC-10 | Agent | Log recommendation with reasoning, track outcome | Must |
| UC-11 | Agent | Query recommendation ledger to review history and debate | Should |
| UC-12 | Agent | Get probable starters and handedness matchups from MLB API | Must |
| UC-13 | Agent | Evaluate keeper-eligible players for future value | Should |
| UC-14 | Agent | View player snippets: yesterday's stats, news, injury, "didn't play" | Must |

## Key Requirements

| ID | Requirement | Source | Priority |
|----|-------------|--------|----------|
| R-01 | Server exposes Fantrax data via MCP tools over streamable HTTP | UC-01..06 | Must |
| R-02 | Credentials (userSecretId) stored in local config, never in source | UC-08 | Must |
| R-03 | Roster tool returns players with position, stats, injury status, eligibility, and player snippets | UC-01, UC-14 | Must |
| R-04 | Standings tool returns category-level breakdown (not just overall rank) | UC-02 | Must |
| R-05 | Matchup tool returns both teams' rosters and scoring projections | UC-03 | Must |
| R-06 | Projections sourced from FanGraphs or equivalent (not Fantrax API -- unavailable) | UC-04 | Must |
| R-07 | Pickup tool suggests adds/drops based on roster needs, projections, and scoring rules | UC-05 | Must |
| R-08 | Lineup tool considers matchups, platoon splits (batter vs pitcher handedness), and rest days | UC-06, UC-12 | Must |
| R-09 | All tools are read-only -- no roster mutations via the API | All | Must |
| R-10 | Server supports concurrent connections (ariadne + vesper) | UC-07 | Must |
| R-11 | Response caching to avoid hitting Fantrax API on every tool call | All | Should |
| R-12 | Multi-league support: auto-discover leagues via getLeagues, handle mixed scoring (H2H, roto, hybrid, custom) | UC-08 | Must |
| R-13 | Morning briefing pipeline: what happened, what to do, hot takes/editorial | UC-09 | Must |
| R-14 | Recommendation ledger: per-agent recommendations with reasoning, outcome tracking, cross-agent debate | UC-10, UC-11 | Must |
| R-15 | MLB API integration for probable starters, box scores, handedness | UC-12 | Must |
| R-16 | Keeper league awareness: keeper eligibility, future value considerations in analysis | UC-13 | Should |
| R-17 | Config supports [[leagues]] array with per-league settings and keeper flag | UC-08, UC-13 | Must |
| R-18 | Shared data layer: one SQLite database, one MCP server; independent analysis per agent | UC-07, UC-10 | Must |
| R-19 | Deployable on goddess cluster (k8s), replicable to other homelabs | Ops | Should |
| R-20 | Player snippets: yesterday's stats, news, injury updates, "didn't play" for no-news | UC-14 | Must |
