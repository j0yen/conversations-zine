//! Cadence intake — reads quarterly monthly cadence records as moment seeds.
//!
//! When `--cadence-monthly` is passed, this module:
//!
//! 1. Invokes `cadence list monthly --since <duration> --produced-by letter-curate --json`
//!    to enumerate pre-curated monthly records.
//! 2. For each record, reads `record.path` and splits the Markdown content into
//!    paragraph-level chunks ≤ 300 chars, scored by Markdown emphasis and
//!    position-in-letter.
//! 3. Returns the top-K candidate moments (default 15 per record) tagged with
//!    origin `"cadence-monthly"`.
//!
//! The caller merges these into the existing JSONL-derived moment pool.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::Deserialize;

use crate::{Moment, ZineError};

/// Configuration for cadence intake.
#[derive(Debug, Clone)]
pub struct CadenceIntakeConfig {
    /// Duration window to look back for monthly records (e.g. `"92d"`).
    pub since: String,
    /// Maximum candidate moments to pull from each monthly record.
    pub top_k: usize,
}

impl Default for CadenceIntakeConfig {
    fn default() -> Self {
        Self {
            since: "92d".to_string(),
            top_k: 15,
        }
    }
}

/// Result of a cadence intake pass — moments plus the source record IDs.
#[derive(Debug, Default)]
pub struct CadenceIntakeResult {
    /// Candidate moments extracted from monthly records.
    pub moments: Vec<Moment>,
    /// ULID/ID strings of the monthly records that were consumed.
    pub source_ids: Vec<String>,
    /// Count of monthly records that were skipped (path missing / unreadable).
    pub skipped: usize,
}

// ---------- raw cadence JSON shapes ----------

#[derive(Debug, Deserialize)]
struct CadenceRecord {
    id: String,
    path: Option<String>,
    #[allow(dead_code)] // reserved for future use (summary display)
    summary: Option<String>,
}

// ---------- public entry point ----------

/// Pull monthly cadence records and convert them into candidate moments.
///
/// On a completely empty substrate (no records) this is a graceful no-op:
/// it returns an empty [`CadenceIntakeResult`] after emitting one warning
/// to stderr.
///
/// # Errors
/// - [`ZineError::Io`] if the `cadence` subprocess cannot be spawned or read.
pub fn ingest(cfg: &CadenceIntakeConfig) -> Result<CadenceIntakeResult, ZineError> {
    let records = list_monthly_records(&cfg.since)?;
    if records.is_empty() {
        let _ = writeln!(
            std::io::stderr(),
            "zine: --cadence-monthly: no monthly records found within {}; \
             proceeding with JSONL walk only",
            cfg.since
        );
        return Ok(CadenceIntakeResult::default());
    }

    let mut result = CadenceIntakeResult::default();
    for rec in records {
        let path = match rec.path.as_deref() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                result.skipped = result.skipped.saturating_add(1);
                continue;
            }
        };
        match read_moments_from_letter(Path::new(&path), &rec.id, cfg.top_k) {
            Ok(moments) => {
                result.source_ids.push(rec.id);
                result.moments.extend(moments);
            }
            Err(_) => {
                result.skipped = result.skipped.saturating_add(1);
            }
        }
    }
    Ok(result)
}

// ---------- subprocess: cadence list monthly ----------

fn list_monthly_records(since: &str) -> Result<Vec<CadenceRecord>, ZineError> {
    let output = Command::new("cadence")
        .args([
            "list",
            "monthly",
            "--since",
            since,
            "--produced-by",
            "letter-curate",
            "--json",
        ])
        .output()?;
    if output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    // cadence list --json emits a JSON array; tolerate parse failures gracefully.
    let records: Vec<CadenceRecord> =
        serde_json::from_slice(&output.stdout).unwrap_or_default();
    Ok(records)
}

// ---------- markdown parsing ----------

/// Score a paragraph chunk for moment quality.
///
/// Signals:
/// - Emphasis count (bold `**`/`__` and italic `*`/`_`).
/// - Position: chunks in the first half of the letter score slightly higher.
/// - Length sweet-spot: penalties for very short (< 40 chars) or very long
///   (> 300 chars) chunks.
#[allow(clippy::float_arithmetic)]
fn score_chunk(text: &str, emphasis_count: u32, position_frac: f64) -> f64 {
    let len = text.len();
    if len < 20 {
        return 0.0;
    }
    // Length score: peaks at ~150 chars.
    let len_capped = u32::try_from(len.min(500)).unwrap_or(u32::MAX);
    let x = f64::from(len_capped);
    let sigma = 120.0_f64;
    let diff = x - 150.0_f64;
    let len_s = (-(diff * diff) / (2.0_f64 * sigma * sigma)).exp() * 2.0_f64;

    // Emphasis signal.
    let emph_s = f64::from(emphasis_count.min(5)) * 0.4_f64;

    // Position signal: first quarter scores 0.5, last quarter 0.0.
    let pos_s = (1.0_f64 - position_frac).max(0.0_f64) * 0.5_f64;

    len_s + emph_s + pos_s
}

/// Parse `path` (a Markdown letter file) and return up to `top_k` candidate
/// moments scored by the emphasis+position heuristic.
#[allow(clippy::too_many_lines)]
fn read_moments_from_letter(
    path: &Path,
    record_id: &str,
    top_k: usize,
) -> Result<Vec<Moment>, ZineError> {
    let raw = std::fs::read_to_string(path)?;
    let chunks = extract_chunks(&raw);
    let total = chunks.len().max(1);

    let mut scored: Vec<(f64, usize, String, u32)> = chunks
        .into_iter()
        .enumerate()
        .map(|(i, (text, emph))| {
            #[allow(clippy::as_conversions, clippy::cast_precision_loss, clippy::float_arithmetic)]
            let frac = i as f64 / total as f64;
            let s = score_chunk(&text, emph, frac);
            (s, i, text, emph)
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);

    let moments: Vec<Moment> = scored
        .into_iter()
        .map(|(score, idx, text, emph)| Moment {
            session_id: format!("cadence-monthly:{record_id}"),
            turn_index: idx,
            ts: String::new(), // monthly records don't have per-chunk timestamps
            user_text: String::new(), // source is a letter, not a user turn
            assistant_text: text,
            score,
            why: format!(
                "source=cadence-monthly emphasis={emph} chunk-idx={idx}"
            ),
        })
        .collect();

    Ok(moments)
}

/// Walk the Markdown AST and collect (text, `emphasis_count`) pairs for each
/// paragraph / heading / blockquote leaf block, capped at 300 chars.
fn extract_chunks(markdown: &str) -> Vec<(String, u32)> {
    let parser = Parser::new(markdown);
    let mut chunks: Vec<(String, u32)> = Vec::new();
    let mut current = String::new();
    let mut emph: u32 = 0;
    let mut in_emphasis = false;

    for event in parser {
        match event {
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::BlockQuote(_)) => {
                current.clear();
                emph = 0;
                in_emphasis = false;
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::BlockQuote(_),
            ) => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() && trimmed.len() <= 300 {
                    chunks.push((trimmed, emph));
                }
                current.clear();
                emph = 0;
                in_emphasis = false;
            }
            Event::Start(Tag::Emphasis | Tag::Strong) => {
                in_emphasis = true;
            }
            Event::End(TagEnd::Emphasis | TagEnd::Strong) => {
                in_emphasis = false;
            }
            Event::Text(t) => {
                if current.len().saturating_add(t.len()) <= 300 {
                    current.push_str(&t);
                    if in_emphasis {
                        emph = emph.saturating_add(1);
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if !current.is_empty() && !current.ends_with(' ') {
                    current.push(' ');
                }
            }
            Event::Code(t) => {
                if current.len().saturating_add(t.len()).saturating_add(2) <= 300 {
                    current.push('`');
                    current.push_str(&t);
                    current.push('`');
                }
            }
            _ => {}
        }
    }
    chunks
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_LETTER: &str = r#"# Monthly Letter — 2026-04

## Highlights

This was a **productive** month. The team shipped three major features.

The _cadence_ substrate is now live and records are flowing.

## Challenges

Some integration tests were flaky; fixed with a timeout tweak.

## Quote of the month

> The best code is code you don't have to write.
"#;

    #[test]
    fn extract_chunks_finds_paragraphs() {
        let chunks = extract_chunks(FIXTURE_LETTER);
        assert!(!chunks.is_empty(), "should extract at least one chunk");
        // The 'Highlights' heading should appear.
        let texts: Vec<&str> = chunks.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("productive")),
            "should find the productive paragraph; got: {texts:?}"
        );
    }

    #[test]
    fn extract_chunks_emphasis_counted() {
        let chunks = extract_chunks(FIXTURE_LETTER);
        // The paragraph with **productive** should have emphasis > 0.
        let prod = chunks
            .iter()
            .find(|(t, _)| t.contains("productive"));
        assert!(prod.is_some(), "productive paragraph not found");
        let (_, emph) = prod.unwrap();
        assert!(*emph > 0, "expected emphasis count > 0 for bold text");
    }

    #[test]
    fn score_chunk_penalises_short() {
        let short_score = score_chunk("hi", 0, 0.0);
        assert_eq!(short_score, 0.0, "very short text should score 0");
    }

    #[test]
    fn score_chunk_rewards_emphasis() {
        let no_emph = score_chunk("This is a medium length paragraph about Rust.", 0, 0.5);
        let with_emph =
            score_chunk("This is a medium length paragraph about Rust.", 3, 0.5);
        assert!(
            with_emph > no_emph,
            "emphasis should increase score: {with_emph} vs {no_emph}"
        );
    }

    #[test]
    fn read_moments_from_fixture_letter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("letter-2026-04.md");
        std::fs::write(&path, FIXTURE_LETTER).unwrap();

        let moments = read_moments_from_letter(&path, "ulid-test-0001", 5).unwrap();
        assert!(
            !moments.is_empty(),
            "should extract at least one moment from fixture"
        );
        assert!(
            moments.len() <= 5,
            "should respect top_k=5 limit; got {}",
            moments.len()
        );
        // All moments must reference the cadence source.
        for m in &moments {
            assert!(
                m.session_id.starts_with("cadence-monthly:"),
                "session_id should start with cadence-monthly:"
            );
            assert!(
                m.why.contains("source=cadence-monthly"),
                "why should mention source"
            );
        }
    }

    #[test]
    fn ingest_empty_substrate_is_graceful() {
        // cadence list will return no records (substrate is empty in CI).
        // ingest() should return Ok with empty moments — not an error.
        let cfg = CadenceIntakeConfig {
            since: "1s".to_string(),
            top_k: 5,
        };
        let result = ingest(&cfg).unwrap();
        // Either empty (no records) or has some moments — both are valid.
        // The key property is: no panic, no error.
        let _ = result.moments.len();
    }
}
