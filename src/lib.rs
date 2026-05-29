//! conversations-zine — moment extractor for Claude Code session transcripts.
//!
//! Walks `*.jsonl` files in a Claude Code project directory, pairs user/assistant
//! turns, scores each pair with a weighted interest heuristic, and surfaces
//! the top-N ranked moments as JSON or skimmable text.
//!
//! ## Cadence intake (v0.2)
//!
//! Pass `--cadence-monthly` to also pull the quarter's monthly cadence records
//! (produced by `letter-curate`) as pre-curated moment seeds, merged into the
//! same moment pool as the JSONL walk.  Use `--cadence-record` to register the
//! zine output as a `quarterly` cadence record.

#![cfg_attr(not(test), forbid(unsafe_code))]

pub mod cadence_intake;

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use walkdir::WalkDir;

/// Errors returned by the extractor.
#[derive(Debug, Error)]
pub enum ZineError {
    /// Root directory is missing or yielded no candidate JSONL files.
    #[error("no jsonl files found under {0}")]
    NoJsonlFiles(PathBuf),
    /// Failed to parse a `--since` duration like `7d`, `30d`, `12h`.
    #[error("invalid --since duration {0:?} (expected e.g. 7d, 30d, 12h)")]
    InvalidDuration(String),
    /// I/O failure while walking files or reading a transcript.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// System clock returned a negative duration (extremely unlikely).
    #[error("system clock error: {0}")]
    Time(#[from] std::time::SystemTimeError),
}

/// Output format for the extracted moments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Machine-readable JSON (default).
    Json,
    /// Human-readable text suited for piping to `less`.
    Text,
}

/// Knobs controlling extraction.
#[derive(Debug, Clone)]
pub struct ExtractConfig {
    /// Root directory holding `*.jsonl` transcripts.
    pub root: PathBuf,
    /// Window for "recently modified" filter.
    pub since: Duration,
    /// Original `--since` literal (e.g. `90d`) — echoed into output.
    pub since_label: String,
    /// Max number of moments to return.
    pub limit: usize,
    /// When `true`, keep assistant turns with empty text (tool-only).
    pub include_tool_only: bool,
    /// When `true`, only emit pairs where the assistant's next turn apologises.
    pub errors_only: bool,
    /// Output format.
    pub format: Format,
    /// When `Some`, also pull cadence monthly records as moment seeds.
    pub cadence: Option<cadence_intake::CadenceIntakeConfig>,
}

/// A single ranked moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Moment {
    /// Session id (JSONL basename without extension).
    pub session_id: String,
    /// 0-based index of this user/assistant pair within the session's pair list.
    pub turn_index: usize,
    /// Timestamp of the user message (RFC3339).
    pub ts: String,
    /// Concatenated user text content.
    pub user_text: String,
    /// Concatenated assistant text content.
    pub assistant_text: String,
    /// Combined score (higher = more interesting).
    pub score: f64,
    /// One-sentence breakdown of which signals fired.
    pub why: String,
}

/// Top-level JSON output envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractReport {
    /// Echoes the `--since` literal that was requested.
    pub since: String,
    /// Wall-clock time at which extraction ran (RFC3339).
    pub generated_at: String,
    /// Root directory walked.
    pub root: String,
    /// Total number of user/assistant pairs scanned (before --limit).
    pub total_turns_scanned: usize,
    /// Number of moments returned (≤ `limit`).
    pub returned: usize,
    /// Number of moments that originated from cadence monthly records.
    pub moments_from_cadence: usize,
    /// Number of moments that originated from JSONL walk.
    pub moments_from_jsonl: usize,
    /// ULID/IDs of monthly cadence records that were consumed (empty if none).
    pub cadence_source_ids: Vec<String>,
    /// Ranked moments (descending by score).
    pub moments: Vec<Moment>,
}

/// Parse a duration literal like `90d`, `30d`, `12h`, `45m`.
///
/// # Errors
/// Returns [`ZineError::InvalidDuration`] when the literal is empty,
/// missing a recognised suffix, or contains a non-numeric prefix.
pub fn parse_since(s: &str) -> Result<Duration, ZineError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ZineError::InvalidDuration(s.to_string()));
    }
    let (num_str, mul) = match trimmed.chars().last() {
        Some('d') => (&trimmed[..trimmed.len() - 1], 86_400_u64),
        Some('h') => (&trimmed[..trimmed.len() - 1], 3_600_u64),
        Some('m') => (&trimmed[..trimmed.len() - 1], 60_u64),
        Some('s') => (&trimmed[..trimmed.len() - 1], 1_u64),
        _ => return Err(ZineError::InvalidDuration(s.to_string())),
    };
    let n: u64 = num_str
        .parse()
        .map_err(|_| ZineError::InvalidDuration(s.to_string()))?;
    Ok(Duration::from_secs(n.saturating_mul(mul)))
}

// ---------- internal record model ----------

#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(rename = "type")]
    rec_type: Option<String>,
    message: Option<serde_json::Value>,
    timestamp: Option<String>,
}

#[derive(Debug, Clone)]
enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
struct Turn {
    role: Role,
    text: String,
    ts: String,
}

/// Pull a `Turn` out of a raw JSONL record, or `None` if it isn't a real
/// user/assistant text turn (system reminders, tool-result-only user records,
/// thinking/tool_use-only assistant records all return `None`).
fn extract_turn(rec: &RawRecord) -> Option<Turn> {
    let rtype = rec.rec_type.as_deref()?;
    let role = match rtype {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let msg = rec.message.as_ref()?;
    let ts = rec.timestamp.clone().unwrap_or_default();

    // `message.content` is either a string (raw user input) or an array of
    // typed blocks. Tool results live in array form too — those don't count
    // as real user turns.
    let content = msg.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let mut acc = String::new();
            let mut had_real_text = false;
            for blk in blocks {
                let bt = blk.get("type").and_then(serde_json::Value::as_str);
                match bt {
                    Some("text") => {
                        if let Some(t) = blk.get("text").and_then(serde_json::Value::as_str) {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            acc.push_str(t);
                            had_real_text = true;
                        }
                    }
                    Some("tool_result") => {
                        // Tool results are not user turns at all — bail.
                        return None;
                    }
                    // thinking / tool_use / image / etc. — skip silently.
                    _ => {}
                }
            }
            if !had_real_text && acc.is_empty() {
                String::new()
            } else {
                acc
            }
        }
        _ => return None,
    };

    Some(Turn { role, text, ts })
}

// ---------- per-session pairing ----------

#[derive(Debug, Clone)]
struct Pair {
    user_text: String,
    user_ts: String,
    assistant_text: String,
}

/// Pair user turns with the *next* assistant turn. Skips orphan turns.
fn pair_turns(turns: &[Turn]) -> Vec<Pair> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < turns.len() {
        let Some(user) = turns.get(i) else { break };
        if !matches!(user.role, Role::User) {
            i += 1;
            continue;
        }
        // Find the next assistant turn.
        let mut j = i + 1;
        let mut found: Option<usize> = None;
        while j < turns.len() {
            if let Some(t) = turns.get(j) {
                if matches!(t.role, Role::Assistant) {
                    found = Some(j);
                    break;
                }
                // Another user turn before an assistant — the first user
                // had no reply; drop it and resume from the new user.
                if matches!(t.role, Role::User) {
                    break;
                }
            }
            j += 1;
        }
        match found {
            Some(k) => {
                if let (Some(u), Some(a)) = (turns.get(i), turns.get(k)) {
                    out.push(Pair {
                        user_text: u.text.clone(),
                        user_ts: u.ts.clone(),
                        assistant_text: a.text.clone(),
                    });
                }
                i = k + 1;
            }
            None => {
                i += 1;
            }
        }
    }
    out
}

// ---------- scoring ----------

const LENGTH_WEIGHT: f64 = 1.0;
const REDIRECT_WEIGHT: f64 = 3.0;
const CODE_WEIGHT: f64 = 1.0;
const NOVELTY_WEIGHT: f64 = 2.0;

const REDIRECT_KEYWORDS: &[&str] = &[
    "wait",
    "no actually",
    "stop",
    "hmm",
    "that's wrong",
    "you're wrong",
];

const APOLOGY_KEYWORDS: &[&str] = &["sorry", "my mistake", "you're right"];

/// Gaussian-ish bell curve peaking at 600 chars total.
#[allow(clippy::float_arithmetic, clippy::suboptimal_flops)]
fn length_score(total: usize) -> f64 {
    // f64 conversion guarded by clamp — total is at most usize but our
    // transcripts are well under 2^53 chars.
    let n = u32::try_from(total.min(1_000_000)).unwrap_or(u32::MAX);
    let x = f64::from(n);
    let sigma = 400.0_f64;
    let diff = x - 600.0_f64;
    let exponent = -(diff * diff) / (2.0_f64 * sigma * sigma);
    exponent.exp() * 2.5_f64
}

fn count_code_blocks(text: &str) -> u32 {
    // A fenced code block opens AND closes with ```; count pairs.
    let fences = u32::try_from(text.matches("```").count()).unwrap_or(u32::MAX);
    fences / 2
}

#[allow(clippy::float_arithmetic)]
fn code_score(text: &str) -> f64 {
    let c = count_code_blocks(text);
    f64::from(c) * 1.5_f64
}

fn redirect_score(next_user_text: Option<&str>) -> f64 {
    let Some(t) = next_user_text else {
        return 0.0;
    };
    let lower = t.to_lowercase();
    for kw in REDIRECT_KEYWORDS {
        if lower.contains(kw) {
            return 1.0;
        }
    }
    0.0
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
}

#[allow(clippy::float_arithmetic)]
fn novelty_score(pair_text: &str, seen: &HashSet<String>) -> f64 {
    let toks: Vec<String> = words(pair_text).collect();
    if toks.is_empty() {
        return 0.0;
    }
    let mut new_count = 0_usize;
    for t in &toks {
        if !seen.contains(t) {
            new_count = new_count.saturating_add(1);
        }
    }
    let new_f = u32::try_from(new_count).unwrap_or(u32::MAX);
    let total_f = u32::try_from(toks.len()).unwrap_or(u32::MAX).max(1);
    f64::from(new_f) / f64::from(total_f)
}

fn apology_in(text: &str) -> bool {
    let lower = text.to_lowercase();
    APOLOGY_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

// ---------- main extraction ----------

fn discover_files(root: &std::path::Path, cutoff: SystemTime) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if mtime >= cutoff {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}

fn load_turns(path: &std::path::Path) -> Vec<Turn> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut turns: Vec<Turn> = Vec::new();
    for line in reader.lines() {
        let Ok(l) = line else { continue };
        if l.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<RawRecord>(&l) else {
            continue;
        };
        if let Some(t) = extract_turn(&rec) {
            turns.push(t);
        }
    }
    turns
}

#[allow(clippy::float_arithmetic)]
fn score_pair(
    idx: usize,
    pair: &Pair,
    pairs: &[Pair],
    seen: &HashSet<String>,
) -> (f64, String) {
    let next_user_text = pairs.get(idx.saturating_add(1)).map(|p| p.user_text.as_str());
    let total_len = pair.user_text.len().saturating_add(pair.assistant_text.len());
    let len_s = length_score(total_len) * LENGTH_WEIGHT;
    let red_s = redirect_score(next_user_text) * REDIRECT_WEIGHT;
    let code_s = code_score(&pair.assistant_text) * CODE_WEIGHT;
    let combined_pair_text = format!("{} {}", pair.user_text, pair.assistant_text);
    let nov_s = novelty_score(&combined_pair_text, seen) * NOVELTY_WEIGHT;
    let score = len_s + red_s + code_s + nov_s;
    let why = format!(
        "length={len_s:.2} redirect={red_s:.2} code={code_s:.2} novelty={nov_s:.2}"
    );
    (score, why)
}

fn process_session(
    session_id: &str,
    pairs: &[Pair],
    cfg: &ExtractConfig,
    out: &mut Vec<Moment>,
) {
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, pair) in pairs.iter().enumerate() {
        if !cfg.include_tool_only && pair.assistant_text.trim().is_empty() {
            for w in words(&pair.user_text) {
                seen.insert(w);
            }
            continue;
        }
        if cfg.errors_only {
            let next_assistant_text = pairs
                .get(idx.saturating_add(1))
                .map(|p| p.assistant_text.as_str());
            if !next_assistant_text.is_some_and(apology_in) {
                for w in words(&pair.user_text) {
                    seen.insert(w);
                }
                for w in words(&pair.assistant_text) {
                    seen.insert(w);
                }
                continue;
            }
        }
        let (score, why) = score_pair(idx, pair, pairs, &seen);
        out.push(Moment {
            session_id: session_id.to_string(),
            turn_index: idx,
            ts: pair.user_ts.clone(),
            user_text: pair.user_text.clone(),
            assistant_text: pair.assistant_text.clone(),
            score,
            why,
        });
        let combined = format!("{} {}", pair.user_text, pair.assistant_text);
        for w in words(&combined) {
            seen.insert(w);
        }
    }
}

/// Walk the configured root, score every turn pair, return the top-N.
///
/// When `cfg.cadence` is `Some`, also pulls monthly cadence records as
/// additional moment seeds (graceful no-op if substrate is empty).
///
/// # Errors
/// - [`ZineError::NoJsonlFiles`] when the root is missing OR contains
///   no `*.jsonl` files modified within `since`.
/// - [`ZineError::Io`] on filesystem failures.
/// - [`ZineError::Time`] on system clock anomalies.
pub fn extract(cfg: &ExtractConfig) -> Result<ExtractReport, ZineError> {
    if !cfg.root.exists() {
        return Err(ZineError::NoJsonlFiles(cfg.root.clone()));
    }
    let cutoff = SystemTime::now()
        .checked_sub(cfg.since)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let files = discover_files(&cfg.root, cutoff);
    if files.is_empty() {
        return Err(ZineError::NoJsonlFiles(cfg.root.clone()));
    }

    let mut all_moments: Vec<Moment> = Vec::new();
    let mut total_scanned = 0_usize;
    for path in &files {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let turns = load_turns(path);
        let pairs = pair_turns(&turns);
        total_scanned = total_scanned.saturating_add(pairs.len());
        process_session(&session_id, &pairs, cfg, &mut all_moments);
    }

    let jsonl_moment_count = all_moments.len();

    // Cadence intake — merge monthly record moments into the pool (opt-in).
    let mut cadence_source_ids: Vec<String> = Vec::new();
    let cadence_moment_count;
    if let Some(cad_cfg) = &cfg.cadence {
        // Errors from cadence intake are non-fatal: degrade gracefully.
        let intake = cadence_intake::ingest(cad_cfg).unwrap_or_default();
        cadence_moment_count = intake.moments.len();
        cadence_source_ids = intake.source_ids;
        all_moments.extend(intake.moments);
    } else {
        cadence_moment_count = 0;
    }

    all_moments.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_moments.truncate(cfg.limit);

    let now: DateTime<Utc> = Utc::now();
    let returned = all_moments.len();
    Ok(ExtractReport {
        since: cfg.since_label.clone(),
        generated_at: now.to_rfc3339(),
        root: cfg.root.display().to_string(),
        total_turns_scanned: total_scanned,
        returned,
        moments_from_jsonl: jsonl_moment_count.min(returned),
        moments_from_cadence: cadence_moment_count.min(returned),
        cadence_source_ids,
        moments: all_moments,
    })
}

/// Render a report as human-readable text (AC7 layout).
#[must_use]
pub fn render_text(report: &ExtractReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# conversations-zine — since={} root={} scanned={} returned={}\n",
        report.since, report.root, report.total_turns_scanned, report.returned
    ));
    for m in &report.moments {
        out.push_str("\n--------------------------------------------------------------------------------\n");
        out.push_str(&format!(
            "## {} · turn {} · {} · score {:.2}\n\n",
            m.session_id, m.turn_index, m.ts, m.score
        ));
        for line in m.user_text.lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&m.assistant_text);
        out.push_str("\n\n");
        out.push_str(&format!("*(why: {})*\n", m.why));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_basic() {
        assert_eq!(parse_since("7d").unwrap(), Duration::from_secs(7 * 86_400));
        assert_eq!(parse_since("12h").unwrap(), Duration::from_secs(12 * 3_600));
        assert!(parse_since("").is_err());
        assert!(parse_since("xyz").is_err());
        assert!(parse_since("d").is_err());
    }

    #[test]
    fn length_score_peaks_near_600() {
        let s600 = length_score(600);
        let s50 = length_score(50);
        let s5000 = length_score(5_000);
        assert!(s600 > s50, "600 should beat 50: {s600} vs {s50}");
        assert!(s600 > s5000, "600 should beat 5000: {s600} vs {s5000}");
    }

    #[test]
    fn redirect_detected() {
        assert!(redirect_score(Some("wait that's wrong")) > 0.0);
        assert!(redirect_score(Some("hmm let me think")) > 0.0);
        assert!(redirect_score(Some("looks good thanks")) == 0.0);
        assert!(redirect_score(None) == 0.0);
    }

    #[test]
    fn code_block_counting() {
        assert_eq!(count_code_blocks("plain"), 0);
        assert_eq!(count_code_blocks("```rust\nfn x(){}\n```"), 1);
        assert_eq!(count_code_blocks("```a```\n```b```"), 2);
    }
}
