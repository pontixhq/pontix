//! The `pontix` command line tool.
#![forbid(unsafe_code)]

mod output;
mod plan;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use pontix_core::OdooVersion;

/// Predicts what an Odoo upgrade breaks in custom modules, with evidence
/// from the Odoo source of both versions.
#[derive(Debug, Parser)]
#[command(name = "pontix", version = pontix_core::VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Checks custom modules for an upgrade to the next Odoo version.
    Plan(PlanArgs),
}

/// What `pontix plan` checks, and how it reports.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct PlanArgs {
    /// The Odoo version the modules run on now, such as 18.0.
    #[arg(long = "from", value_name = "VERSION", value_parser = version)]
    pub(crate) from: OdooVersion,
    /// The Odoo version to upgrade to: the next one, such as 19.0.
    #[arg(long, value_name = "VERSION", value_parser = version)]
    pub(crate) to: OdooVersion,
    /// The Odoo source tree of the version upgraded from.
    #[arg(long, value_name = "DIR")]
    pub(crate) core_from: PathBuf,
    /// The Odoo source tree of the version upgraded to.
    #[arg(long, value_name = "DIR")]
    pub(crate) core_to: PathBuf,
    /// A folder of custom modules to check. Repeat it for several.
    #[arg(long, value_name = "DIR", required = true)]
    pub(crate) addons: Vec<PathBuf>,
    /// Only these modules, comma separated. Default: every module in the
    /// addons folders.
    #[arg(long, value_name = "NAMES", value_delimiter = ',')]
    pub(crate) modules: Vec<String>,
    /// How to write the report.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub(crate) format: Format,
    /// Writes the report to this file instead of the screen.
    #[arg(long, value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,
    /// Exits with code 1 when a finding is at least this severe, for CI.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    pub(crate) fail_on: Option<FailOn>,
}

/// How a report is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// For people.
    Text,
    /// For tools and AI agents, with a schema version.
    Json,
}

/// The least severe finding that fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FailOn {
    /// Errors only.
    Error,
    /// Errors and warnings.
    Warning,
}

fn version(written: &str) -> Result<OdooVersion, String> {
    OdooVersion::parse(written)
        .ok_or_else(|| format!("Pontix does not know Odoo {written}; it knows 18.0 and 19.0"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Plan(args) => match plan::run(&args) {
            Ok(outcome) => outcome.exit_code(),
            Err(error) => {
                eprintln!("error: {error}");
                plan::input_error()
            }
        },
    }
}
