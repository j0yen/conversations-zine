//! `zine` CLI — surfaces ranked moments from Claude Code session transcripts.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use conversations_zine::{ExtractConfig, Format, ZineError, extract, parse_since, render_text};

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
        } => run_extract(
            &since,
            root,
            limit,
            &format,
            include_tool_only,
            exclude_tool_only,
            errors_only,
        ),
    }
}

#[allow(clippy::fn_params_excessive_bools, clippy::too_many_arguments)]
fn run_extract(
    since: &str,
    root: Option<PathBuf>,
    limit: usize,
    format: &str,
    include_tool_only: bool,
    exclude_tool_only: bool,
    errors_only: bool,
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

    let cfg = ExtractConfig {
        root: root_path,
        since: since_dur,
        since_label: since.to_string(),
        limit,
        include_tool_only: include,
        errors_only,
        format: fmt,
    };

    match extract(&cfg) {
        Ok(report) => match fmt {
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
        },
        Err(e @ ZineError::NoJsonlFiles(_)) => {
            let _ = writeln!(std::io::stderr(), "zine: {e}");
            ExitCode::from(2)
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "zine: {e}");
            ExitCode::from(1)
        }
    }
}
