# Changelog

All notable changes to `conversations-zine` are documented here.

## [0.2.0] — 2026-05-29

### Added

- `src/cadence_intake.rs` — cadence intake module (~80 LOC) that reads the
  quarter's monthly cadence records (produced by `letter-curate`) as
  pre-curated moment seeds, merges them into the existing JSONL-derived
  moment pool via a shared ranker.
- `--cadence-monthly` flag (default: off) — opt-in to pull past 92 days of
  `monthly` records and parse each for moment-shaped excerpts.
- `--cadence-record` flag (default: on when `--cadence-monthly` is set) —
  registers the zine output as a `quarterly` cadence record via
  `cadence record quarterly`.
- `--cadence-since <duration>` flag (default: `92d`) — window for monthly
  record lookup.
- Graceful no-op when monthly records are absent: one warning to stderr,
  JSONL-walk output is unaffected.
- Full acceptance test suite: `tests/acceptance_ac1.rs` through
  `tests/acceptance_ac8.rs` covering all PRD acceptance criteria.
- Unit tests for markdown parsing, emphasis counting, chunk scoring, and
  fixture-based moment extraction.
- `ExtractReport` fields: `moments_from_cadence`, `moments_from_jsonl`,
  `cadence_source_ids` — support for cadence record with source provenance.

### Changed

- `zine extract` now wires cadence intake into the moment pool when
  `--cadence-monthly` is set.

### Dependencies

- `pulldown-cmark = "0.12"` added (Markdown AST parser for letter chunks).

## [0.1.0] — 2026-05-24

### Added

- Initial `zine extract` command: walks `*.jsonl` transcripts, pairs
  user/assistant turns, scores by length + redirect + code + novelty,
  emits top-N moments as JSON or text.
- `--since`, `--root`, `--limit`, `--format`, `--include-tool-only`,
  `--errors-only` flags.
- 8 acceptance tests (`acceptance_ac1`–`acceptance_ac8`).
