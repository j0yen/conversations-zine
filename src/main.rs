//! `zine` CLI — surfaces ranked moments from Claude Code session transcripts.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use conversations_zine::{
    ExtractConfig, Format, ZineError, cadence_intake::CadenceIntakeConfig, extract, parse_since,
    render_text,
};

#[derive(Debug, Parser)]
#[command(name = "zine", about = "Conversations-zine moment extractor (Phase 0)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Walk a Claude Code project directory and surface ranked candidate moments.
    Extract {
        /// Window of recently-modified transcripts to consider (e.g. 90d, 30d, 12h).
        #[arg(long, default_value = "90d")]
        since: String,

        /// Root directory containing `*.jsonl` transcripts.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Maximum number of ranked moments to emit.
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Output format.
        #[arg(long, default_value = "json")]
        format: String,

        /// Include assistant turns whose text body is empty (tool-only).
        #[arg(long, default_value_t = false)]
        include_tool_only: bool,

        /// Exclude tool-only assistant turns (default; the flag is here so it
        /// can be passed explicitly when a script wants to be unambiguous).
        #[arg(long, default_value_t = false)]
        exclude_tool_only: bool,

        /// Only emit pairs where the assistant's next turn apologises.
        #[arg(long, default_value_t = false)]
        errors_only: bool,

        /// Also pull the quarter's monthly cadence records as moment seeds
        /// (produced by letter-curate). Off by default — opt-in because the
        /// zine's quality is sensitive to source mixing.
        #[arg(long, default_value_t = false)]
        cadence_monthly: bool,

        /// Duration window to look back for monthly records (e.g. 92d).
        /// Only relevant when --cadence-monthly is set.
        #[arg(long, default_value = "92d")]
        cadence_since: String,

        /// Record the zine output as a quarterly cadence record.
        /// Defaults to on when --cadence-monthly is set; off otherwise.
        #[arg(long)]
        cadence_record: Option<bool>,

        /// Path to write (or read) the zine bundle Markdown for cadence recording.
        #[arg(long)]
        cadence_bundle_path: Option<PathBuf>,
    },
}

fn default_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".claude/projects/-home-jsy");
        return p;
    }
    PathBuf::from(".")
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Extract {
            since,
            root,
            limit,
            format,
            include_tool_only,
            exclude_tool_only,
            errors_only,
            cadence_monthly,
            cadence_since,
            cadence_record,
            cadence_bundle_path,
        } => run_extract(
            &since,
            root,
            limit,
            &format,
            include_tool_only,
            exclude_tool_only,
            errors_only,
            cadence_monthly,
            &cadence_since,
            cadence_record,
            cadence_bundle_path.as_deref(),
        ),
    }
}

#[allow(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
fn run_extract(
    since: &str,
    root: Option<PathBuf>,
    limit: usize,
    format: &str,
    include_tool_only: bool,
    exclude_tool_only: bool,
    errors_only: bool,
    cadence_monthly: bool,
    cadence_since: &str,
    cadence_record: Option<bool>,
    cadence_bundle_path: Option<&std::path::Path>,
) -> ExitCode {
    let since_dur = match parse_since(since) {
        Ok(d) => d,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "zine: {e}");
            return ExitCode::from(2);
        }
    };

    // --include-tool-only opts in; --exclude-tool-only is the default and
    // exists so scripts can be explicit. Either way, default = excluded.
    let _ = exclude_tool_only;
    let include = include_tool_only;

    let fmt = match format {
        "json" => Format::Json,
        "text" => Format::Text,
        other => {
            let _ = writeln!(
                std::io::stderr(),
                "zine: unknown --format {other:?} (expected json|text)"
            );
            return ExitCode::from(2);
        }
    };

    let root_path = root.unwrap_or_else(default_root);

    let cadence_cfg = if cadence_monthly {
        Some(CadenceIntakeConfig {
            since: cadence_since.to_string(),
            top_k: 15,
        })
    } else {
        None
    };

    let cfg = ExtractConfig {
        root: root_path,
        since: since_dur,
        since_label: since.to_string(),
        limit,
        include_tool_only: include,
        errors_only,
        format: fmt,
        cadence: cadence_cfg,
    };

    let report = match extract(&cfg) {
        Ok(r) => r,
        Err(e @ ZineError::NoJsonlFiles(_)) => {
            let _ = writeln!(std::io::stderr(), "zine: {e}");
            return ExitCode::from(2);
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "zine: {e}");
            return ExitCode::from(1);
        }
    };

    // Optional cadence recording.
    let should_record = cadence_record.unwrap_or(cadence_monthly);
    if should_record {
        if let Some(bundle_path) = cadence_bundle_path {
            record_quarterly_cadence(&report, bundle_path);
        } else {
            let _ = writeln!(
                std::io::stderr(),
                "zine: --cadence-record requires --cadence-bundle-path <path>"
            );
            return ExitCode::from(2);
        }
    }

    match fmt {
        Format::Json => match serde_json::to_string_pretty(&report) {
            Ok(s) => {
                println!("{s}");
                ExitCode::from(0)
            }
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "zine: serialise failed: {e}");
                ExitCode::from(1)
            }
        },
        Format::Text => {
            print!("{}", render_text(&report));
            ExitCode::from(0)
        }
    }
}

/// Invoke `cadence record quarterly` to register the zine output as a
/// quarterly cadence record.  Non-fatal: errors are printed to stderr only.
fn record_quarterly_cadence(
    report: &conversations_zine::ExtractReport,
    bundle_path: &std::path::Path,
) {
    use std::process::Command;

    let n = report.returned;
    let m = report.moments_from_cadence;
    let k = report.moments_from_jsonl;
    let summary = format!(
        "zine output: {n} moments, {m} from monthly cadence, {k} from JSONL"
    );

    let sources = if report.cadence_source_ids.is_empty() {
        String::new()
    } else {
        report.cadence_source_ids.join(",")
    };

    let mut cmd = Command::new("cadence");
    cmd.args([
        "record",
        "quarterly",
        "--produced-by",
        "zine",
        "--path",
        &bundle_path.display().to_string(),
        "--summary",
        &summary,
        "--meta",
        &format!("moment_count={n}"),
        "--meta",
        &format!("source_mix={m}/{k}"),
    ]);
    if !sources.is_empty() {
        cmd.args(["--sources", &sources]);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let _ = writeln!(
                std::io::stderr(),
                "zine: cadence quarterly record created"
            );
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = writeln!(
                std::io::stderr(),
                "zine: cadence record quarterly failed: {stderr}"
            );
        }
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "zine: could not invoke cadence: {e}"
            );
        }
    }
}
