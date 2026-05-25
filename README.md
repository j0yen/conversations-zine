# conversations-zine

> The quarterly zine's bottleneck is the moment-extractor step — without a CLI that walks session JSONLs and surfaces ~50 ranked candidate excerpts, the editor (the author) has to read every transcript by hand to find moments worth printing.

## Install

### One-liner

```sh
curl -fsSL https://raw.githubusercontent.com/j0yen/conversations-zine/main/install.sh | bash
```

### Manual

```sh
git clone --depth 1 https://github.com/j0yen/conversations-zine.git
cd conversations-zine
./install.sh
```

Installs the `zine` binary via `cargo install --path . --locked`. Requires `cargo` / `rustc 1.85+` and `git`. Built binary lands in `~/.cargo/bin/`.

## Why

The quarterly zine's bottleneck is the moment-extractor step — without a CLI that walks session JSONLs and surfaces ~50 ranked candidate excerpts, the editor (the author) has to read every transcript by hand to find moments worth printing. Phase 0 ships only the extractor; layout/print/mail are explicitly downstream and human-driven.

## Build

```sh
cargo build --release
```

Produces `target/release/zine`. Symlink into `~/.local/bin/` if you want it on `$PATH`.

## Usage

```sh
zine --help
```

## Audience

the author at quarter-end, running the extractor against the last 90 days of Claude Code session JSONLs (~/.claude/projects/-home-the author/*.jsonl), reading the ranked candidates in terminal, and using the JSON to seed manual selection. Audience for the JSON: the author reading it before manual curation, no downstream automation in Phase 0.

## Acceptance criteria

This project was scaffolded from a PRD via the `autobuilder` pipeline. The MUST-level acceptance criteria are:

- **AC1**: `zine extract --since <duration> [--root <jsonl-dir>] [--limit <N>] [--format json|text]` walks all *.jsonl files modified within the duration (e.g. 90d, 30d), extracts conversation turns, scores each turn pair (user + assistant) with th...
- **AC2**: JSON output shape: top-level `{since, generated_at, root, total_turns_scanned, returned, moments: [...]}`. Each moment: `{session_id, turn_index, ts, user_text, assistant_text, score, why}`. `why` is a one-sentence string explaining the ...
- **AC3**: Interest heuristic is a weighted sum: (a) length bell curve peaking ~600 chars total, (b) user-redirect keywords in turn N+1 (`wait`, `no actually`, `stop`, `hmm`) — indicates surprise, (c) code blocks in the assistant text (positive wei...
- **AC4**: Default --root is `~/.claude/projects/-home-jsy/` (the Claude Code project transcripts directory). Non-existent root → exit 2 with stderr containing the path and `no jsonl files found`.
- **AC5**: Turn pairs are extracted from the JSONL by walking records and pairing a `user` role record with the next `assistant` role record. Records that don't fit the pair (system reminders, tool calls in isolation) are skipped, not paired.
- **AC6**: `--exclude-tool-only` (default true) drops assistant turns whose text content is empty (tool-only invocations). `--include-tool-only` includes them.

Each AC has a matching integration test under `tests/acceptance_ac<n>.rs`.

## Provenance

Built via the [`autobuilder`](https://github.com/j0yen/autobuilder) pipeline (PRD intake -> intent-card -> scaffold -> iterate-and-prove). Originally consolidated as a subdir of the [`wintermute`](https://github.com/j0yen/wintermute) monorepo; this standalone repo is a fresh-init snapshot for easier consumption and distribution.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
