use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use gaal::error::GaalError;
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "gaal", version, about = "Agent session observability CLI")]
struct Cli {
    /// Human-readable output (otherwise JSON).
    #[arg(short = 'H', long = "human", global = true)]
    human: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Fleet view across sessions.
    Ls {
        /// Filter by engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Lower bound: duration/date (for example: 1d, 2026-03-01).
        #[arg(long)]
        since: Option<String>,
        /// Upper bound date/time.
        #[arg(long)]
        before: Option<String>,
        /// Substring match on working directory.
        #[arg(long)]
        cwd: Option<String>,
        /// Filter by tag (repeatable, AND logic).
        #[arg(long)]
        tag: Vec<String>,
        /// Sort field.
        #[arg(long, value_enum, default_value_t = LsSort::Started)]
        sort: LsSort,
        /// Max number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Return aggregate totals instead of individual sessions.
        #[arg(long)]
        aggregate: bool,
    },


    /// Session details with optional focused views (formerly show).
    Inspect {
        /// Session ID or ID prefix. Use `latest` to resolve the newest session.
        id: Option<String>,
        /// File ops view; when passed without a value, defaults to "all".
        #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "all")]
        files: Option<InspectFiles>,
        /// Errors and non-zero exits only.
        #[arg(long)]
        errors: bool,
        /// Commands only.
        #[arg(long)]
        commands: bool,
        /// Git operations only.
        #[arg(long)]
        git: bool,
        /// Include all arrays and fields (full output).
        #[arg(short = 'F', long)]
        full: bool,
        /// Token usage breakdown.
        #[arg(long)]
        tokens: bool,
        /// Full event timeline.
        #[arg(long)]
        trace: bool,
        /// Raw JSONL source path.
        #[arg(long)]
        source: bool,
        /// Render as session markdown (full conversation flow).
        #[arg(long)]
        markdown: bool,
        /// Batch IDs in comma-delimited form.
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
        /// Batch filter by tag.
        #[arg(long)]
        tag: Vec<String>,
    },

    /// Inverted query: which session did X to Y.
    #[command(
        after_long_help = "Available verbs:\n  read       Files opened with the Read tool\n  wrote      Files created/modified with Write or Edit tool\n  ran        Commands executed via Bash tool (matches program names)\n  touched    Any file interaction (read + wrote combined)\n  changed    Files modified (wrote + edited, excludes read-only)\n  deleted    File deletions (rm commands and file removals)"
    )]
    Who {
        /// Action verb (read|wrote|ran|touched|changed|deleted).
        verb: Option<String>,
        /// Target file/path/command pattern.
        target: Option<String>,
        /// Time window lower bound.
        #[arg(long, default_value = "7d")]
        since: String,
        /// Upper bound date/time.
        #[arg(long)]
        before: Option<String>,
        /// Restrict by working directory.
        #[arg(long)]
        cwd: Option<String>,
        /// Restrict by engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Restrict by tag (repeatable).
        #[arg(long)]
        tag: Vec<String>,
        /// For `ran`, only non-zero command exits.
        #[arg(long)]
        failed: bool,
        /// Max number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Show full per-fact output including detail fields.
        #[arg(short = 'F', long)]
        full: bool,
    },

    /// Full-text search over indexed facts.
    Search {
        /// Search query.
        query: String,
        /// Time window lower bound.
        #[arg(long, default_value = "30d")]
        since: String,
        /// Restrict by working directory.
        #[arg(long)]
        cwd: Option<String>,
        /// Restrict by engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Restrict to a specific content field.
        #[arg(long, value_enum, default_value_t = SearchField::All)]
        field: SearchField,
        /// Context lines around each match.
        #[arg(long, default_value_t = 2)]
        context: usize,
        /// Max number of results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Semantic session retrieval (eywa replacement).
    Recall {
        /// Optional topic query.
        query: Option<String>,
        /// Recency window in days.
        #[arg(long = "days-back", default_value_t = 14)]
        days_back: u32,
        /// Max number of sessions.
        #[arg(long, default_value_t = 3)]
        limit: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = RecallFormat::Brief)]
        format: RecallFormat,
        /// Minimum substance score.
        #[arg(long, default_value_t = 1)]
        substance: u8,
    },

    /// Generate a random salt token for session identification.
    Salt,

    /// Find the first JSONL file containing the provided salt token.
    #[command(name = "find-salt")]
    FindSalt {
        /// Salt token to search for.
        salt: String,
    },

    /// Generate/create a session handoff markdown via LLM extraction.
    #[command(name = "create-handoff")]
    CreateHandoff {
        /// Session ID (or "today").
        #[arg(required = false)]
        id: Option<String>,
        /// Explicit JSONL file path to use.
        #[arg(long)]
        jsonl: Option<PathBuf>,
        /// LLM engine for extraction.
        #[arg(long)]
        engine: Option<Engine>,
        /// LLM model for extraction.
        #[arg(long)]
        model: Option<String>,
        /// Custom prompt path.
        #[arg(long)]
        prompt: Option<String>,
        /// Provider backend.
        #[arg(long, value_enum, default_value_t = Provider::AgentMux)]
        provider: Provider,
        /// Output format identifier.
        #[arg(long, default_value = "eywa-compatible")]
        format: String,
        /// Run batch mode.
        #[arg(long)]
        batch: bool,
        /// Time window lower bound.
        #[arg(long, default_value = "7d")]
        since: String,
        /// Max concurrent batch workers.
        #[arg(long, default_value_t = 1, value_parser = parse_parallel)]
        parallel: usize,
        /// Minimum turns required for batch candidates.
        #[arg(long, default_value_t = 3)]
        min_turns: usize,
        /// Extract the current (nearest) detected session instead of preferring a parent session.
        #[arg(long)]
        this: bool,
        /// Preview candidates without processing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Index maintenance and backfill operations.
    Index {
        #[command(subcommand)]
        cmd: IndexCommand,
    },

    /// Apply or remove tags on a session.
    Tag {
        /// Session ID.
        id: String,
        /// Tags to add/remove (not used with `gaal tag ls`).
        tags: Vec<String>,
        /// Remove tags instead of adding them.
        #[arg(long)]
        remove: bool,
    },
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Index all existing JSONL files.
    Backfill {
        /// Restrict backfill to one engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Lower bound date/time.
        #[arg(long)]
        since: Option<String>,
        /// Re-index even if already indexed.
        #[arg(long)]
        force: bool,
        /// Also generate session markdown files during backfill.
        #[arg(long)]
        with_markdown: bool,
        /// Write session markdowns to this directory (YYYY/MM/DD/<short-id>.md).
        /// Implies --with-markdown. Skips active sessions and existing files.
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },
    /// Show index health/status.
    Status,
    /// Force re-index of one session.
    Reindex {
        /// Session ID.
        id: String,
    },
    /// Import legacy eywa handoff-index data.
    ImportEywa {
        /// Optional path to handoff-index.json.
        path: Option<String>,
    },
    /// Remove old facts before a date.
    Prune {
        /// Upper-bound date (required).
        #[arg(long)]
        before: String,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum Engine {
    Claude,
    Codex,
}

#[derive(Clone, Debug, ValueEnum)]
enum LsSort {
    Started,
    Ended,
    Tokens,
    Cost,
    Duration,
}

#[derive(Clone, Debug, ValueEnum)]
enum InspectFiles {
    Read,
    Write,
    All,
}

#[derive(Clone, Debug, ValueEnum)]
enum SearchField {
    Prompts,
    Replies,
    Commands,
    Errors,
    Files,
    All,
}

#[derive(Clone, Debug, ValueEnum)]
enum RecallFormat {
    Summary,
    Handoff,
    Brief,
    Full,
    Eywa,
}

#[derive(Clone, Debug, ValueEnum)]
enum Provider {
    #[value(name = "agent-mux")]
    AgentMux,
    Openrouter,
}

fn main() {
    let cli = Cli::parse();

    if let Err(err) = run(cli) {
        emit_json_error(&err);
        std::process::exit(err.exit_code());
    }
}

fn run(cli: Cli) -> Result<(), GaalError> {
    let Cli { human, command } = cli;

    match command {
        Commands::Ls {
            engine,
            since,
            before,
            cwd,
            tag,
            sort,
            limit,
            aggregate,
        } => {
            let args = gaal::commands::ls::LsArgs {
                engine: engine.map(convert_ls_engine),
                since,
                before,
                cwd,
                tag,
                sort: Some(convert_ls_sort(sort)),
                limit: usize_to_i64("limit", limit)?,
                aggregate,
                human_readable: human,
            };
            gaal::commands::ls::run(args)
        }
        Commands::Inspect {
            id,
            files,
            errors,
            commands,
            git,
            full,
            tokens,
            trace,
            source,
            markdown,
            ids,
            tag,
        } => {
            let args = gaal::commands::inspect::InspectArgs {
                id,
                files: files.map(convert_inspect_files),
                errors,
                commands,
                git,
                full,
                tokens,
                trace,
                source,
                markdown,
                ids: csv_or_none(ids),
                tag: single_or_none("--tag", tag)?,
                human,
            };
            gaal::commands::inspect::run(args)
        }
        Commands::Who {
            verb,
            target,
            since,
            before,
            cwd,
            engine,
            tag,
            failed,
            limit,
            full,
        } => {
            let args = gaal::commands::who::WhoArgs {
                verb: verb.unwrap_or_default(),
                target,
                since,
                before,
                cwd,
                engine: engine.map(convert_engine_string),
                tag: single_or_none("--tag", tag)?,
                failed,
                limit: usize_to_i64("limit", limit)?,
                human,
                full,
            };
            gaal::commands::who::run(args)
        }
        Commands::Search {
            query,
            since,
            cwd,
            engine,
            field,
            context,
            limit,
        } => {
            let args = gaal::commands::search::SearchArgs {
                query,
                since,
                cwd,
                engine: engine.map(convert_engine_string),
                field: convert_search_field(field),
                context,
                limit,
                human,
            };
            gaal::commands::search::run(args)
        }
        Commands::Recall {
            query,
            days_back,
            limit,
            format,
            substance,
        } => {
            let args = gaal::commands::recall::RecallArgs {
                query,
                days_back: i64::from(days_back),
                limit,
                format: convert_recall_format(format),
                substance: i32::from(substance),
                human,
            };
            gaal::commands::recall::run(args)
        }
        Commands::Salt => gaal::commands::salt::run(),
        Commands::FindSalt { salt } => {
            let args = gaal::commands::find::FindArgs { salt };
            gaal::commands::find::run(args)
        }
        Commands::CreateHandoff {
            id,
            jsonl,
            engine,
            model,
            prompt,
            provider,
            format,
            batch,
            since,
            parallel,
            min_turns,
            this,
            dry_run,
        } => {
            let args = gaal::commands::handoff::HandoffArgs {
                id,
                jsonl,
                engine: engine.map(convert_engine_string),
                model,
                prompt: prompt.map(PathBuf::from),
                provider: Some(convert_provider(provider)),
                format: Some(format),
                batch,
                since: Some(since),
                parallel,
                min_turns,
                force_this: this,
                dry_run,
            };
            gaal::commands::handoff::run(args)
        }
        Commands::Index { cmd } => match cmd {
            IndexCommand::Backfill {
                engine,
                since,
                force,
                with_markdown,
                output_dir,
            } => {
                let args = gaal::commands::index::BackfillArgs {
                    engine: engine.map(convert_engine_string),
                    since,
                    force,
                    with_markdown,
                    output_dir,
                };
                gaal::commands::index::run_backfill(args)
            }
            IndexCommand::Status => gaal::commands::index::run_status(),
            IndexCommand::Reindex { id } => {
                let args = gaal::commands::index::ReindexArgs { id };
                gaal::commands::index::run_reindex(args)
            }
            IndexCommand::ImportEywa { path } => {
                let args = gaal::commands::index::ImportEywaArgs { path };
                gaal::commands::index::run_import_eywa(args)
            }
            IndexCommand::Prune { before } => {
                let args = gaal::commands::index::PruneArgs { before };
                gaal::commands::index::run_prune(args)
            }
        },
        Commands::Tag { id, tags, remove } => {
            let args = gaal::commands::tag::TagArgs { id, tags, remove };
            gaal::commands::tag::run(args)
        }
    }
}

fn convert_ls_engine(engine: Engine) -> gaal::commands::ls::LsEngine {
    match engine {
        Engine::Claude => gaal::commands::ls::LsEngine::Claude,
        Engine::Codex => gaal::commands::ls::LsEngine::Codex,
    }
}

fn convert_ls_sort(sort: LsSort) -> gaal::commands::ls::LsSort {
    match sort {
        LsSort::Started => gaal::commands::ls::LsSort::Started,
        LsSort::Ended => gaal::commands::ls::LsSort::Ended,
        LsSort::Tokens => gaal::commands::ls::LsSort::Tokens,
        LsSort::Cost => gaal::commands::ls::LsSort::Cost,
        LsSort::Duration => gaal::commands::ls::LsSort::Duration,
    }
}

fn convert_inspect_files(mode: InspectFiles) -> gaal::commands::inspect::FilesMode {
    match mode {
        InspectFiles::Read => gaal::commands::inspect::FilesMode::Read,
        InspectFiles::Write => gaal::commands::inspect::FilesMode::Write,
        InspectFiles::All => gaal::commands::inspect::FilesMode::All,
    }
}

fn convert_search_field(field: SearchField) -> gaal::commands::search::SearchField {
    match field {
        SearchField::Prompts => gaal::commands::search::SearchField::Prompts,
        SearchField::Replies => gaal::commands::search::SearchField::Replies,
        SearchField::Commands => gaal::commands::search::SearchField::Commands,
        SearchField::Errors => gaal::commands::search::SearchField::Errors,
        SearchField::Files => gaal::commands::search::SearchField::Files,
        SearchField::All => gaal::commands::search::SearchField::All,
    }
}

fn convert_recall_format(format: RecallFormat) -> gaal::commands::recall::RecallFormat {
    match format {
        RecallFormat::Summary => gaal::commands::recall::RecallFormat::Summary,
        RecallFormat::Handoff => gaal::commands::recall::RecallFormat::Handoff,
        RecallFormat::Brief => gaal::commands::recall::RecallFormat::Brief,
        RecallFormat::Full => gaal::commands::recall::RecallFormat::Full,
        RecallFormat::Eywa => gaal::commands::recall::RecallFormat::Eywa,
    }
}

fn convert_provider(provider: Provider) -> String {
    match provider {
        Provider::AgentMux => "agent-mux".to_string(),
        Provider::Openrouter => "openrouter".to_string(),
    }
}

fn convert_engine_string(engine: Engine) -> String {
    match engine {
        Engine::Claude => "claude".to_string(),
        Engine::Codex => "codex".to_string(),
    }
}

fn usize_to_i64(field: &str, value: usize) -> Result<i64, GaalError> {
    i64::try_from(value)
        .map_err(|_| GaalError::ParseError(format!("{field} is too large: {value}")))
}

fn parse_parallel(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("invalid --parallel value `{raw}`"))?;
    if (1..=5).contains(&value) {
        Ok(value)
    } else {
        Err(format!("invalid --parallel value `{raw}` (expected 1..=5)"))
    }
}

fn csv_or_none(values: Vec<String>) -> Option<String> {
    if values.is_empty() {
        None
    } else {
        Some(values.join(","))
    }
}

fn single_or_none(flag: &str, values: Vec<String>) -> Result<Option<String>, GaalError> {
    match values.len() {
        0 => Ok(None),
        1 => Ok(values.into_iter().next()),
        _ => Err(GaalError::ParseError(format!(
            "{flag} accepts a single value in this command implementation"
        ))),
    }
}

fn emit_json_error(err: &GaalError) {
    let payload = json!({
        "ok": false,
        "error": err.to_string(),
        "exit_code": err.exit_code()
    });
    eprintln!("{payload}");
}
