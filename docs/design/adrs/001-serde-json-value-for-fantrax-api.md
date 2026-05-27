# ADR-001: Use serde_json::Value for Fantrax API responses

**Date:** 2026-05-26
**Status:** Accepted

## Context

The Fantrax beta API is undocumented. Response shapes vary between endpoints and may change without notice. Attempting to define strongly-typed structs up front would require reverse-engineering every response variant, and those structs would break as the API evolves.

We need a deserialization strategy that lets us move fast — ship working integrations now and tighten the types later as we gain confidence in the response shapes.

## Decision

Deserialize all Fantrax API responses to `serde_json::Value`. Extract fields using helper methods that navigate the JSON tree and return `Option<T>` or domain-specific error types when expected fields are missing.

## Consequences

- **Flexible:** New endpoints can be integrated without defining new types. Shape changes don't cause deserialization panics.
- **No compile-time guarantees:** Field access errors surface at runtime, not compile time. Tests and integration checks must compensate.
- **Migration path:** As response shapes stabilize through usage, we will incrementally introduce typed structs (with `#[serde(deny_unknown_fields)]` where appropriate) to reclaim compile-time safety.
- **Helper methods become the contract:** The extraction helpers effectively document the expected shape — keep them well-tested.
