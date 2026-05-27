# ADR-005: Recommendation ledger as first-class feature

**Date:** 2026-05-26
**Status:** Accepted

## Context

Ariadne and vesper are independent agents that will each make fantasy baseball recommendations — start/sit, add/drop, trade targets. Mira wants these agents to track their own recommendations, measure outcomes, and eventually debate each other's calls. "The real friends were the takes we made along the way."

Without a structured record, recommendations are ephemeral chat messages — impossible to score, compare, or learn from.

## Decision

Implement a recommendation ledger as a SQLite table (using the shared database from ADR-002). Each entry records: `agent_id`, timestamp, recommendation type, player(s) involved, reasoning, confidence level, and outcome fields (filled in retrospectively). The ledger is a first-class feature, not an afterthought logging table.

## Consequences

- **Accountability:** Every recommendation is recorded with reasoning. Agents can be evaluated on their track record, not just their latest take.
- **Debate history:** When agents disagree, the ledger provides the structured basis for comparison. "Ariadne said start him, vesper said sit — who was right?" becomes a queryable question.
- **Emergent personality:** Over time, each agent's recommendation history shapes its identity. Ariadne's hot-take tendencies vs. vesper's conservative leanings become visible through data.
- **Retrospective scoring:** Outcome tracking requires a mechanism to revisit recommendations after games complete. This ties into the morning briefing pipeline — "last night's results" can automatically score open recommendations.
- **Schema complexity:** The ledger table needs to handle multiple recommendation types (start/sit, add/drop, trade) with different field requirements. We use a flexible schema with a JSON reasoning column rather than over-normalizing.
