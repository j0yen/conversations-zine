# conversations-zine

Walks a quarter of Claude Code session transcripts and surfaces the ~50 most interesting moments, ranked, so the editor reads candidates instead of every transcript by hand.

## Why it exists

The quarterly zine has one bottleneck: finding the moments worth printing. Without a tool to read them, the editor — the author — has to re-read 90 days of session JSONLs by hand. `zine extract` does the reading: it walks the transcripts, scores each user/assistant turn pair on an interest heuristic, and emits the top candidates as JSON to seed manual selection. Phase 0 ships only the extractor. Layout, print, and mail are downstream and human-driven by design.

## Install

```sh
# One-liner
curl -fsSL https://raw.githubusercontent.com/j0yen/conversations-zine/main/install.sh | bash

# Or from a clone
git clone --depth 1 https://github.com/j0yen/conversations-zine.git
cd conversations-zine && ./install.sh
```

`install.sh` runs `cargo install --path . --locked`, so the `zine` binary lands in `~/.cargo/bin/`. Requires `cargo` / `rustc 1.85+` and `git`.

## Quickstart

```sh
# Top 50 moments from the last 90 days, as JSON
zine extract

# Read them in the terminal instead
zine extract --since 30d --limit 20 --format text
```

The default root is `~/.claude/projects/-home-jsy/`; pass `--root <dir>` to point elsewhere. A non-existent root exits 2 with `no jsonl files found` and the path on stderr.

The JSON output has the shape:

```
{ since, generated_at, root, total_turns_scanned, returned, moments: [ … ] }
```

Each moment carries `session_id`, `turn_index`, `ts`, `user_text`, `assistant_text`, `score`, and a one-sentence `why` explaining the rank.

## How the ranking works

A turn pair is one `user` record paired with the next `assistant` record; records that don't pair (isolated system reminders, tool-only calls) are skipped. By default, assistant turns with an empty text body — tool-only invocations — are dropped (`--include-tool-only` keeps them; `--exclude-tool-only` states the default explicitly for scripts).

The interest score is a weighted sum of four signals:

| Signal | Weight | Why it scores |
|---|---|---|
| Length bell curve, peaking ~600 chars total | 1.0 | Too short says little; too long is a wall. |
| User-redirect keywords in the next turn (`wait`, `no actually`, `stop`, `hmm`) | 3.0 | A correction marks a surprising moment. |
| Code blocks in the assistant text | 1.0 | Something was built, not just discussed. |
| Novelty | 2.0 | Reward the unusual over the routine. |

`--errors-only` narrows to pairs where the assistant's next turn apologises.

## Cadence intake (opt-in)

`--cadence-monthly` also pulls the quarter's monthly cadence records (produced by `letter-curate`) as additional moment seeds. It's off by default — the zine's quality is sensitive to mixing sources, so blending is a deliberate choice. `--cadence-record` writes the zine output back as a quarterly cadence record (on by default when `--cadence-monthly` is set); `--cadence-bundle-path` sets where the bundle Markdown is written or read. `--cadence-since` (default `92d`) is the look-back window for the monthly records.

## Build and test

```sh
cargo build --release   # produces target/release/zine
cargo test
```

Each MUST-level acceptance criterion has a matching integration test under `tests/acceptance_ac<n>.rs`.

## Status

Phase 0: the extractor only. Built via the [autobuilder](https://github.com/j0yen/autobuilder) pipeline (PRD → intent-card → scaffold → iterate-and-prove). Originally a subdirectory of the [wintermute](https://github.com/j0yen/wintermute) monorepo; this standalone repo is a fresh-init snapshot for easier distribution.

## License

Dual MIT / Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
