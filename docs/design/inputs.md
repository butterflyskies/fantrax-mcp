# Design inputs

Captured from design conversation with Mira, 2026-05-26.

## League landscape

Mira has **6 leagues**, corrected from an initial assumption of 1:

- 3 daily-lineup leagues
- 1 weekly H2H league
- 2 best ball / worst ball leagues

Scoring formats are mixed: H2H categories, roto, hybrid, and custom variants. One league is a keeper league where future value affects roster decisions.

## Scoring and analysis

- **v1 is arithmetic** — weighted category scores, platoon splits, recent trends
- **Bayesian framework slots in later** via a trait boundary (no rewrite, just swap the implementation)
- **Platoon logic is the immediate practical win** — start/sit decisions based on pitcher/batter handedness matchups against probable starters

## Morning briefing

Three layers:

1. **What happened** — last night's results, how your roster performed, category movement
2. **What to do** — lineup decisions, waiver wire targets, trade opportunities
3. **Hot takes** — editorial commentary with personality

Personality direction: "talk radio host but less annoying and racist." Opinionated, entertaining, informed — not a neutral stat dump.

## Agent architecture

- **Two agents:** ariadne and vesper
- **Shared data layer** (SQLite) — both agents read from the same stats, standings, and roster data
- **Independent analysis** — each agent runs its own scoring and generates its own recommendations
- **Recommendation ledger** — structured record of every recommendation with reasoning, confidence, and outcome tracking
- **Agent debate** — "the real friends were the takes we made along the way." Agents can disagree, and the ledger makes it possible to score who was right

## Infrastructure

- **Secrets custody:** 1Password shared vault, pulled via ExternalSecrets into k8s pod environment variables. No secrets in config files or memory.
- **Deployment:** Goddess cluster (primary), replicable to Mira's homelab
- **Mira is authorized to steer directly** — can direct agent behavior, adjust priorities, override recommendations

## Config

- `[[leagues]]` TOML array with per-league settings
- Can be seeded from Fantrax `getLeagues` API for auto-discovery
- League-specific overrides for scoring weights, keeper rules, lineup type
