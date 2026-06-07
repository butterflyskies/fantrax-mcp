# ADR-006: Multi-league config with auto-discovery

**Date:** 2026-05-26
**Status:** Accepted

## Context

Initial design assumed a single league. Mira actually has 6 leagues with different formats:

- 3 daily-lineup leagues
- 1 weekly H2H league
- 2 best ball / worst ball leagues

Scoring formats span H2H categories, roto, hybrid, and custom variants. One league is a keeper league with future-value considerations that affect add/drop and trade analysis.

A single-league config would force users to run separate instances or hack around the limitation. The config needs to handle real-world usage from day one.

## Decision

Use a `[[leagues]]` TOML array in the config file. Each league entry specifies its Fantrax league ID, format (H2H, roto, hybrid, best-ball, worst-ball), lineup type (daily/weekly), scoring categories, and keeper settings. League entries can be manually authored or seeded from the Fantrax `getLeagues` API endpoint.

## Consequences

- **Handles real-world usage:** Mira's 6-league setup works without workarounds.
- **Auto-discovery:** The `getLeagues` API can populate the `[[leagues]]` array, reducing manual config for users with many leagues. League-specific settings (custom weights, keeper rules) still require manual tuning.
- **Config complexity:** The TOML file is more complex than a single-league config. We mitigate this with sensible defaults — most fields are optional, and format-specific settings only apply to their format type.
- **Analysis implications:** The scoring engine (ADR-004) must be parameterized by league config. A player might be a strong start in one league format and a sit in another. Briefings must be league-aware.
- **Keeper league:** Future-value considerations introduce a time dimension to recommendations. The trait boundary (ADR-004) accommodates this as a scoring strategy variant.
