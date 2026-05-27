<!-- design-meta
status: draft
last-updated: 2026-05-26
phase: 1
-->

# Problem Space: Fantrax Fantasy Baseball MCP Server

## What are we solving?

Mira manages a fantasy baseball roster on Fantrax but has been neglecting it during a difficult stretch. She wants AI agents (ariadne and vesper) to independently analyze her roster, suggest waiver wire pickups, and optimize lineups — then compare notes.

Currently there's no way for an MCP-connected agent to access Fantrax data. The agents can discuss baseball but can't see the actual roster, standings, matchups, or available players.

## Inputs and outputs

**Inputs:**
- Fantrax beta API: roster state, standings, scoring rules, matchup schedules, player IDs
- Third-party sources: player projections (FanGraphs, Rotowire, Baseball Savant)
- User config: league ID, team ID, userSecretId

**Outputs:**
- Current roster with player stats and status
- Lineup optimization suggestions (who to start/bench given matchups)
- Waiver wire recommendations (who to pick up, who to drop)
- Matchup analysis (strengths/weaknesses vs opponent)
- Standings context (what categories to target)

**Key transformations:**
- Raw Fantrax API data → structured roster/matchup state
- Projections + roster state + scoring rules → optimization recommendations
- Waiver wire pool + roster gaps + projections → pickup suggestions

## Boundaries

**In scope:**
- MCP server exposing Fantrax data as tools
- Roster analysis and recommendation logic
- Multi-agent access (ariadne + vesper independently)

**Out of scope:**
- Automated roster moves (read-only — humans make the final call)
- Draft assistance (season is in progress)
- Trade analysis (v2 maybe)
- Other sports (baseball only for now)

## Success criteria

- Mira can ask "who should I pick up this week?" and get a data-backed answer
- Two agents can independently analyze and compare recommendations
- Weekly check-in cadence is sustainable without manual data entry
