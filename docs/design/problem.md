<!-- design-meta
status: draft
last-updated: 2026-05-26
phase: 1
-->

# Problem Space: Fantrax Fantasy Baseball MCP Server

## What are we solving?

Mira manages fantasy baseball rosters across **six leagues** on Fantrax but has been neglecting them during a difficult stretch. She wants AI agents (ariadne and vesper) to independently analyze her rosters, suggest waiver wire pickups, and optimize lineups — then compare notes and debate.

Currently there's no way for an MCP-connected agent to access Fantrax data. The agents can discuss baseball but can't see the actual rosters, standings, matchups, or available players.

### League landscape

| Type | Count | Lineup cadence | Scoring style |
|------|-------|----------------|---------------|
| Daily lineup | 3 | Set daily | Varies (H2H, roto, hybrid) |
| Weekly H2H | 1 | Set weekly | Head-to-head categories |
| Best ball / worst ball | 2 | Auto-optimized | Custom scoring rules |

One of the six is a **keeper league** with keeper-eligible players and future value considerations — players who may be mediocre now but worth holding for next season.

Scoring is mixed across leagues: H2H categories, rotisserie, hybrid, and custom rules. The server must handle all of these.

## Inputs and outputs

**Inputs:**
- Fantrax beta API: roster state, standings, scoring rules, matchup schedules, player IDs, league discovery (getLeagues)
- MLB API: probable starters, box scores, batter/pitcher handedness
- Third-party sources: player projections (FanGraphs, Rotowire, Baseball Savant)
- User config: userSecretId, with auto-discovery of leagues via getLeagues endpoint

**Outputs:**
- Current roster with player stats, status, and player snippets (yesterday's line, news, injuries, "didn't play")
- Lineup optimization suggestions (who to start/bench given matchups and platoon splits)
- Waiver wire recommendations (who to pick up, who to drop)
- Matchup analysis (strengths/weaknesses vs opponent)
- Standings context (what categories to target)
- **Morning briefing** with three layers:
  1. **What happened** — overnight results, player performances, injury news
  2. **What to do** — lineup moves, waiver claims, urgent actions across all leagues
  3. **Hot takes / editorial** — agent opinions, trends, bold calls
- **Recommendation ledger** — ariadne and vesper independently recommend, log their reasoning, track outcomes, and debate each other's calls

**Key transformations:**
- Raw Fantrax API data → structured roster/matchup state per league
- MLB API data → probable starters, handedness matchups, box scores
- Projections + roster state + scoring rules → optimization recommendations
- Waiver wire pool + roster gaps + projections → pickup suggestions
- Daily platoon logic: batter handedness vs probable starter handedness → start/sit adjustments
- Recommendation history + outcomes → recommendation ledger with win/loss tracking

## Boundaries

**In scope:**
- MCP server exposing Fantrax data as tools
- Multi-league support with auto-discovery and mixed scoring rules
- Roster analysis and recommendation logic
- Morning briefing pipeline
- Recommendation ledger (per-agent reasoning, outcome tracking, cross-agent debate)
- MLB API integration for daily game context (probable starters, handedness, box scores)
- Daily platoon logic (batter handedness vs starter handedness)
- Keeper league considerations (future value, keeper eligibility)
- Multi-agent access (ariadne + vesper share data layer, independent analysis)
- Deployment on goddess cluster (k8s)
- Shared data layer: one MCP server, one SQLite database

**Out of scope:**
- Automated roster moves (read-only — humans make the final call)
- Draft assistance (season is in progress)
- Trade analysis (v2 maybe)
- Other sports (baseball only for now)

## Stakeholder authorization

Mira is authorized to steer the project directly — she can request features, reprioritize, and make design decisions without routing through Lina.

## Replicability

Architecture should be replicable to Mira's homelab later. Keep deployment concerns modular and avoid hard-coding goddess-specific assumptions.

## Success criteria

- Mira can ask "who should I pick up this week?" and get a data-backed answer across all six leagues
- Morning briefing covers overnight results, required actions, and editorial takes
- Two agents independently recommend, log reasoning, and track outcomes
- Agents can debate each other's recommendations using the ledger
- Platoon-aware lineup suggestions (handedness matchups from MLB API)
- Keeper league analysis factors in future value, not just current production
- Weekly check-in cadence is sustainable without manual data entry
