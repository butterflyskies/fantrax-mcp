# ADR-003: MLB Stats API for game context

**Date:** 2026-05-26
**Status:** Accepted

## Context

Fantasy baseball analysis requires game-level context that Fantrax does not provide: probable starting pitchers, pitcher/batter handedness, live and historical boxscores, and schedule data. This information drives platoon logic (start/sit decisions based on handedness matchups) and powers the morning briefing's "what happened last night" layer.

## Decision

Use the MLB Stats API (`statsapi.mlb.com`) as the source of truth for game context. The API is free, requires no authentication, and provides comprehensive coverage of schedules, lineups, player metadata, and game results.

## Consequences

- **Third external dependency:** Adds a second HTTP integration alongside Fantrax. Failure modes now include MLB API downtime or rate limiting.
- **Reliable and well-documented:** Unlike Fantrax's beta API, the MLB Stats API has stable endpoints, consistent response shapes, and community documentation. We can use typed structs here from the start.
- **Comprehensive:** Covers probable pitchers, handedness, boxscores, standings, and schedule — everything needed for platoon logic and briefings without scraping.
- **Caching opportunity:** Game results are immutable once final. Aggressive caching in SQLite (ADR-002) reduces API calls and speeds up briefing generation.
