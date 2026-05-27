# ADR-004: Trait-based CategoryScorer for analysis engine

**Date:** 2026-05-26
**Status:** Accepted

## Context

The v1 analysis engine uses straightforward arithmetic: weighted category scores, platoon splits, recent performance trends. Mira wants to bring a Bayesian framework later — prior distributions on player performance, posterior updates from observed stats, credible intervals for uncertainty-aware recommendations.

The scoring strategy needs to be swappable without rewriting the recommendation pipeline, briefing generator, or agent integration layer.

## Decision

Define the analysis engine behind a `CategoryScorer` trait boundary. The trait specifies the interface for scoring players across fantasy categories. V1 implements this with arithmetic scoring; a future Bayesian implementation can slot in by implementing the same trait.

## Consequences

- **Clean upgrade path:** Bayesian scoring can be developed and tested independently, then swapped in with a single type change at the composition root.
- **Slight over-abstraction for v1:** The trait boundary adds indirection that v1 alone does not need. We accept this cost because the Bayesian upgrade is a stated goal, not speculative.
- **Testability:** Both implementations can be tested against the same trait contract, making it easy to verify that the Bayesian scorer produces reasonable results before switching.
- **Strategy pattern:** Additional scoring strategies (e.g., opponent-aware H2H scoring, keeper league future-value adjustments) can be composed or layered on top of the base trait.
