use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use gaal::error::GaalError;
use serde_json::json;

#[derive(Debug, Parser)]
#[command(
    name = "gaal",
    version,
    about = "Agent session observability CLI",
    after_help = "New here?\n  gaal onboard --dry-run     Explain skill install and first launch\n  gaal onboard               Guided setup"
)]
struct Cli {
    /// Human-readable output (otherwise JSON).
    #[arg(short = 'H', long = "human", global = true)]
    human: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Explain agent-facing setup after package installation.
    Onboard {
        /// Explain the setup flow without implying any local changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Fleet view across sessions.
    Ls {
        /// Filter by engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Filter by session type.
        #[arg(long, value_enum)]
        session_type: Option<SessionTypeFilter>,
        /// Filter by subagent type (e.g. gsd-heavy, gsd-coordinator, Explore).
        #[arg(long)]
        subagent_type: Option<String>,
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
        /// Show all sessions including noise (0 tool calls and <30s duration).
        #[arg(long)]
        all: bool,
        /// Include subagent sessions. Deprecated; sessions are included by default.
        #[arg(long, hide = true)]
        include_subagents: bool,
        /// Hide subagent sessions and show only standalone/coordinator sessions.
        #[arg(long, conflicts_with = "include_subagents")]
        skip_subagents: bool,
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
        /// Include empty/low-signal subagents in coordinator views.
        #[arg(long)]
        include_empty: bool,
        /// Batch IDs in comma-delimited form.
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
        /// Batch filter by tag.
        #[arg(long)]
        tag: Vec<String>,
    },

    /// Get session transcript markdown (replaces inspect --markdown).
    #[command(
        after_long_help = "Examples:\n  gaal transcript latest\n  gaal transcript 249aad1e\n  gaal transcript latest --stdout\n  gaal transcript latest --force"
    )]
    Transcript {
        /// Session ID or ID prefix. Use `latest` for newest session.
        id: Option<String>,
        /// Re-render even if cached file exists.
        #[arg(long)]
        force: bool,
        /// Dump markdown to stdout instead of returning file path as JSON.
        #[arg(long)]
        stdout: bool,
    },

    /// Render source-backed transcript slices across a time window.
    #[command(
        after_long_help = "Examples:\n  gaal activity --since 1d\n  gaal activity --since 2026-05-25 --before 2026-05-26 --stdout\n  gaal activity --session 9ad81c91 --since 2026-05-25 --before 2026-05-26\n  gaal activity --engine codex --since 7d"
    )]
    Activity {
        /// Lower bound: duration/date/RFC3339. Default: 1d.
        #[arg(long, default_value = "1d")]
        since: String,
        /// Upper bound date/time. Default: now.
        #[arg(long)]
        before: Option<String>,
        /// Restrict by engine.
        #[arg(long)]
        engine: Option<Engine>,
        /// Restrict by working directory substring.
        #[arg(long)]
        cwd: Option<String>,
        /// Render one resolved session only.
        #[arg(long)]
        session: Option<String>,
        /// Hide subagent sessions.
        #[arg(long)]
        skip_subagents: bool,
        /// Re-render even if cached activity exists.
        #[arg(long)]
        force: bool,
        /// Dump markdown to stdout instead of returning file path as JSON.
        #[arg(long)]
        stdout: bool,
        /// Max DB candidates to render.
        #[arg(long, default_value_t = 250)]
        limit: usize,
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
        query: Option<String>,
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

    /// Ranked handoff retrieval for continuity.
    Recall {
        /// Optional topic query.
        query: Option<String>,
        /// Direct handoff lookup by session ID (bypasses search). Supports prefix, `latest`.
        #[arg(long)]
        id: Option<String>,
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

    /// Resolve a short session ID to paths and metadata.
    Resolve {
        /// Short session ID (8-char prefix).
        id: Option<String>,
        /// Filter by engine to disambiguate.
        #[arg(long)]
        engine: Option<Engine>,
    },

    /// Generate a random salt token for session identification.
    Salt,

    /// Find the first JSONL file containing the provided salt token.
    #[command(name = "find-salt")]
    FindSalt {
        /// Salt token to search for.
        salt: Option<String>,
        /// Restrict search to one source engine.
        #[arg(long)]
        engine: Option<SaltEngine>,
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
        /// Worker engine for handoff extraction, not source-session detection.
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
        #[arg(long, default_value = "markdown")]
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
        /// Compatibility no-op while parent-session preference is disabled.
        #[arg(long)]
        this: bool,
        /// Preview candidates without processing.
        #[arg(long)]
        dry_run: bool,
        /// Effort level (low, medium, high, xhigh). Overrides config.
        #[arg(long, value_parser = parse_effort)]
        effort: Option<String>,
    },

    /// Index maintenance and backfill operations.
    Index {
        #[command(subcommand)]
        cmd: IndexCommand,
    },

    /// Apply or remove tags on a session.
    Tag {
        /// Session ID (or `ls` to list all tags).
        id: Option<String>,
        /// Tags to add/remove (not used with `gaal tag ls`).
        tags: Vec<String>,
        /// Remove tags instead of adding them.
        #[arg(long)]
        remove: bool,
    },
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Index supported local agent trace artifacts into SQLite + Tantivy.
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
    /// Remove old facts before a date.
    Prune {
        /// Upper-bound date (required).
        #[arg(long)]
        before: String,
    },
    /// Recover orphaned subagent files whose parent JSONL was deleted.
    #[command(name = "recover-orphans")]
    RecoverOrphans {
        /// Preview what would be recovered without writing to the database.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum Engine {
    Claude,
    Codex,
    Gemini,
    Agy,
    Hermes,
    Grok,
}

#[derive(Clone, Debug, ValueEnum)]
enum SaltEngine {
    Claude,
    Codex,
    Agy,
    Grok,
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
}

#[derive(Clone, Debug, ValueEnum)]
enum Provider {
    #[value(name = "agent-mux")]
    AgentMux,
    Openrouter,
}

#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "lower")]
enum SessionTypeFilter {
    Coordinator,
    Standalone,
    Subagent,
}

fn main() {
    let cli = Cli::parse();
    let human = cli.human;
    let command = current_command_name();

    if let Err(err) = run(cli) {
        emit_error(&err, human, command);
        std::process::exit(err.exit_code());
    }
}

fn run(cli: Cli) -> Result<(), GaalError> {
    let Cli { human, command } = cli;

    match command {
        Commands::Onboard { dry_run } => run_onboard(dry_run, human),
        Commands::Ls {
            engine,
            session_type,
            subagent_type,
            since,
            before,
            cwd,
            tag,
            sort,
            limit,
            aggregate,
            all,
            include_subagents,
            skip_subagents,
        } => {
            let args = gaal::commands::ls::LsArgs {
                engine: engine.map(convert_ls_engine),
                session_type: session_type.map(|st| match st {
                    SessionTypeFilter::Coordinator => "coordinator".to_string(),
                    SessionTypeFilter::Standalone => "standalone".to_string(),
                    SessionTypeFilter::Subagent => "subagent".to_string(),
                }),
                since,
                before,
                cwd,
                tag,
                sort: Some(convert_ls_sort(sort)),
                limit: usize_to_i64("limit", limit)?,
                aggregate,
                human_readable: human,
                all,
                include_subagents,
                skip_subagents,
                subagent_type,
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
            include_empty,
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
                include_empty,
                ids: csv_or_none(ids),
                tag: single_or_none("--tag", tag)?,
                human,
            };
            gaal::commands::inspect::run(args)
        }
        Commands::Transcript { id, force, stdout } => {
            let args = gaal::commands::transcript::TranscriptArgs {
                id,
                force,
                stdout,
                human,
            };
            gaal::commands::transcript::run(args)
        }
        Commands::Activity {
            since,
            before,
            engine,
            cwd,
            session,
            skip_subagents,
            force,
            stdout,
            limit,
        } => {
            let args = gaal::commands::activity::ActivityArgs {
                since,
                before,
                engine: engine.map(convert_engine_string),
                cwd,
                session,
                skip_subagents,
                force,
                stdout,
                limit: usize_to_i64("limit", limit)?,
                human,
            };
            gaal::commands::activity::run(args)
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
            let query = query
                .ok_or_else(|| GaalError::ParseError("search query cannot be empty".to_string()))?;
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
            id,
            days_back,
            limit,
            format,
            substance,
        } => {
            let args = gaal::commands::recall::RecallArgs {
                query,
                id,
                days_back: i64::from(days_back),
                limit,
                format: convert_recall_format(format),
                substance: i32::from(substance),
                human,
            };
            gaal::commands::recall::run(args)
        }
        Commands::Resolve { id, engine } => {
            let id = id.ok_or_else(|| {
                GaalError::ParseError("resolve requires a session ID".to_string())
            })?;
            let args = gaal::commands::resolve::ResolveArgs {
                id,
                engine: engine.map(convert_engine_string),
                human,
            };
            gaal::commands::resolve::run(args)
        }
        Commands::Salt => gaal::commands::salt::run(),
        Commands::FindSalt { salt, engine } => {
            let salt = salt.ok_or_else(|| {
                GaalError::ParseError("find-salt requires a salt token".to_string())
            })?;
            let args = gaal::commands::find::FindArgs {
                salt,
                human,
                engine: engine.map(convert_salt_engine_string),
            };
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
            effort,
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
                effort,
                human,
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
            IndexCommand::Status => gaal::commands::index::run_status(human),
            IndexCommand::Reindex { id } => {
                let args = gaal::commands::index::ReindexArgs { id };
                gaal::commands::index::run_reindex(args)
            }
            IndexCommand::Prune { before } => {
                let args = gaal::commands::index::PruneArgs { before };
                gaal::commands::index::run_prune(args)
            }
            IndexCommand::RecoverOrphans { dry_run } => {
                let args = gaal::commands::index::RecoverOrphansArgs { dry_run };
                gaal::commands::index::run_recover_orphans(args)
            }
        },
        Commands::Tag { id, tags, remove } => {
            let id = id.ok_or_else(|| {
                GaalError::ParseError("tag requires a session id or `ls`".to_string())
            })?;
            let args = gaal::commands::tag::TagArgs { id, tags, remove };
            gaal::commands::tag::run(args)
        }
    }
}

fn run_onboard(dry_run: bool, human: bool) -> Result<(), GaalError> {
    const SKILL_URL: &str = "https://github.com/buildoak/gaal/tree/master/skill";
    const SKILL_MD_URL: &str = "https://github.com/buildoak/gaal/blob/master/skill/SKILL.md";
    const FIRST_RUN_URL: &str =
        "https://github.com/buildoak/gaal/blob/master/skill/references/first-run.md";

    let first_launch = [
        "gaal index backfill",
        "gaal index status",
        "gaal ls -H --limit 5",
    ];
    let update_commands = [
        "brew upgrade buildoak/tap/gaal",
        "cargo install gaal-cli --force",
    ];
    let optional_after = [
        "gaal create-handoff latest --dry-run",
        "agent-mux config prompts",
        "agent-mux config engines --json",
    ];

    if human {
        println!("Gaal onboarding");
        println!();
        if dry_run {
            println!("Dry run: this command explains setup only. It does not write files, install skills, index sessions, schedule jobs, install agent-mux, or generate handoffs.");
            println!();
        }
        println!("Agent install instruction:");
        println!("  1. Use the latest bundled Gaal skill from:");
        println!("     {SKILL_URL}");
        println!("  2. Read the first-run reference before acting:");
        println!("     {FIRST_RUN_URL}");
        println!("  3. Run the first local index:");
        for cmd in first_launch {
            println!("     {cmd}");
        }
        println!();
        println!("Approval boundaries:");
        println!("  - Skill/reference installation into agent-specific folders is explicit.");
        println!("  - Scheduled indexing is explicit and must run only `gaal index backfill`.");
        println!("  - agent-mux setup is optional; real handoff generation starts with dry-run.");
        println!();
        println!("Optional after the first index:");
        for cmd in optional_after {
            println!("  {cmd}");
        }
        println!();
        println!("Existing install update:");
        for cmd in update_commands {
            println!("  {cmd}");
        }
        println!("  gaal onboard --dry-run");
        return Ok(());
    }

    let payload = json!({
        "ok": true,
        "kind": "onboarding",
        "dry_run": dry_run,
        "summary": "Package installation only puts gaal on PATH. The installing or updating agent should fetch the latest Gaal skill/reference from the repo, then run the first local index.",
        "no_side_effects": [
            "does_not_write_skill_files",
            "does_not_index_sessions",
            "does_not_install_scheduled_jobs",
            "does_not_install_agent_mux",
            "does_not_generate_handoffs"
        ],
        "skill": {
            "directory_url": SKILL_URL,
            "skill_md_url": SKILL_MD_URL,
            "first_run_reference_url": FIRST_RUN_URL,
            "instruction": "Install or load the latest skill directory from the GitHub repo path supported by the active agent harness. If the harness has no skill installer, read SKILL.md and skill/references/first-run.md into context before running setup commands."
        },
        "update": {
            "binary_commands": update_commands,
            "skill_instruction": "Package managers update the gaal binary, not copied agent skills. Refresh the local Gaal skill from the skill.directory_url through the active harness's supported skill-install mechanism, or read SKILL.md plus skill/references/first-run.md into context if no installer exists.",
            "then_run": "gaal onboard --dry-run"
        },
        "first_launch": {
            "commands": first_launch,
            "zero_session_state": "If ls reports no indexed sessions after backfill, the install can still be healthy; supported agent traces may not exist on this machine yet."
        },
        "approval_boundaries": [
            "Installing skill/reference material into Codex, Claude, or agent-mux folders is explicit.",
            "Scheduled indexing is explicit and must run only `gaal index backfill`.",
            "agent-mux setup is optional and requires direct approval.",
            "Real handoff generation must start with `gaal create-handoff ... --dry-run`."
        ],
        "optional_after_first_index": optional_after
    });

    gaal::output::json::print_json(&payload).map_err(GaalError::from)
}

fn convert_ls_engine(engine: Engine) -> gaal::commands::ls::LsEngine {
    match engine {
        Engine::Claude => gaal::commands::ls::LsEngine::Claude,
        Engine::Codex => gaal::commands::ls::LsEngine::Codex,
        Engine::Gemini => gaal::commands::ls::LsEngine::Gemini,
        Engine::Agy => gaal::commands::ls::LsEngine::Agy,
        Engine::Hermes => gaal::commands::ls::LsEngine::Hermes,
        Engine::Grok => gaal::commands::ls::LsEngine::Grok,
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
        Engine::Gemini => "gemini".to_string(),
        Engine::Agy => "agy".to_string(),
        Engine::Hermes => "hermes".to_string(),
        Engine::Grok => "grok".to_string(),
    }
}

fn convert_salt_engine_string(engine: SaltEngine) -> String {
    match engine {
        SaltEngine::Claude => "claude".to_string(),
        SaltEngine::Codex => "codex".to_string(),
        SaltEngine::Agy => "agy".to_string(),
        SaltEngine::Grok => "grok".to_string(),
    }
}

fn usize_to_i64(field: &str, value: usize) -> Result<i64, GaalError> {
    i64::try_from(value)
        .map_err(|_| GaalError::ParseError(format!("{field} is too large: {value}")))
}

fn parse_effort(raw: &str) -> Result<String, String> {
    match raw {
        "low" | "medium" | "high" | "xhigh" => Ok(raw.to_string()),
        _ => Err(format!(
            "invalid --effort value `{raw}` (valid: low, medium, high, xhigh)"
        )),
    }
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

fn current_command_name() -> &'static str {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-H" | "--human" => continue,
            "ls" => return "ls",
            "inspect" => return "inspect",
            "transcript" => return "transcript",
            "activity" => return "activity",
            "who" => return "who",
            "search" => return "search",
            "recall" => return "recall",
            "resolve" => return "resolve",
            "onboard" => return "onboard",
            "salt" => return "salt",
            "find-salt" => return "find-salt",
            "create-handoff" => return "create-handoff",
            "index" => return "index",
            "tag" => return "tag",
            _ => continue,
        }
    }
    "gaal"
}

fn emit_error(err: &GaalError, human: bool, command: &str) {
    if human {
        eprintln!("{}", err.format_human(command));
    } else {
        emit_json_error(err, command);
    }
}

fn emit_json_error(err: &GaalError, command: &str) {
    let payload = err.format_json(command);
    eprintln!("{payload}");
}
