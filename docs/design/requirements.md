<!-- design-meta
status: draft
last-updated: 2026-05-26
phase: 2
-->

# Requirements: Fantrax Fantasy Baseball MCP Server

## Use Cases

| ID | Actor | Use Case | Priority |
|----|-------|----------|----------|
| UC-01 | Agent | View current roster with player stats and injury status | Must |
| UC-02 | Agent | View league standings and category breakdowns | Must |
| UC-03 | Agent | View current matchup and opponent's roster | Must |
| UC-04 | Agent | Get player projections for the week | Must |
| UC-05 | Agent | Suggest waiver wire pickups based on roster gaps | Must |
| UC-06 | Agent | Suggest optimal lineup for upcoming games | Should |
| UC-07 | Agent | Compare two agents' independent recommendations | Should |
| UC-08 | User | Configure league/team credentials | Must |

## Key Requirements

| ID | Requirement | Source | Priority |
|----|-------------|--------|----------|
| R-01 | Server exposes Fantrax data via MCP tools over streamable HTTP | UC-01..06 | Must |
| R-02 | Credentials (userSecretId, leagueId) stored in local config, never in source | UC-08 | Must |
| R-03 | Roster tool returns players with position, stats, injury status, eligibility | UC-01 | Must |
| R-04 | Standings tool returns category-level breakdown (not just overall rank) | UC-02 | Must |
| R-05 | Matchup tool returns both teams' rosters and scoring projections | UC-03 | Must |
| R-06 | Projections sourced from FanGraphs or equivalent (not Fantrax API — unavailable) | UC-04 | Must |
| R-07 | Pickup tool suggests adds/drops based on roster needs, projections, and scoring rules | UC-05 | Must |
| R-08 | Lineup tool considers matchups, platoon splits, and rest days | UC-06 | Should |
| R-09 | All tools are read-only — no roster mutations via the API | All | Must |
| R-10 | Server supports concurrent connections (ariadne + vesper) | UC-07 | Should |
| R-11 | Response caching to avoid hitting Fantrax API on every tool call | All | Should |
