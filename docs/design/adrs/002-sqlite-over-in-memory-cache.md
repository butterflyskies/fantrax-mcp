# ADR-002: SQLite over in-memory cache

**Date:** 2026-05-26
**Status:** Accepted

## Context

Two agents (ariadne and vesper) need to share a data layer — league standings, player stats, roster snapshots, and a recommendation ledger. The data must persist across process restarts and support concurrent reads from independent agent sessions.

An in-memory `HashMap` would be simplest but fails on persistence and cross-process sharing. A full database server (Postgres) would be overkill for what is fundamentally a single-node, read-heavy workload.

## Decision

Use `rusqlite` with the `bundled` feature (statically links SQLite). Store the database file on disk with WAL mode enabled for concurrent read access.

## Consequences

- **Persistent:** Data survives restarts. No warm-up cost or cache-miss storms on startup.
- **Concurrent-read safe:** WAL mode allows multiple readers without blocking. Single-writer constraint is acceptable — agents rarely write simultaneously.
- **Recommendation ledger:** SQLite is a natural fit for the structured ledger (ADR-005) — queryable, transactional, and auditable.
- **Slightly more complex:** Requires schema management and migration strategy. We accept this cost for the durability and sharing guarantees.
- **Bundled trade-off:** Increases binary size slightly but eliminates system SQLite version dependency — important for container and homelab portability.
