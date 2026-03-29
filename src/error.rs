use thiserror::Error;

/// Errors returned by Gaal operations.
#[derive(Debug, Error)]
pub enum GaalError {
    /// No matching results were found.
    #[error("no results")]
    NoResults,
    /// The provided ID matched multiple sessions.
    #[error("ambiguous id: {0}")]
    AmbiguousId(String),
    /// The requested entity was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// The SQLite index does not exist yet.
    #[error("index not found; run `gaal index backfill`")]
    NoIndex,
    /// Parsing failed for user input or source data.
    #[error("parse error: {0}")]
    ParseError(String),
    /// Filesystem I/O failure.
    #[error(transparent)]
    Io(std::io::Error),
    /// SQLite database failure.
    #[error(transparent)]
    Db(rusqlite::Error),
    /// Internal logic error (e.g. serialization, data format).
    #[error("{0}")]
    Internal(String),
    /// Invalid configuration or config loading failure.
    #[error("config error: {0}")]
    Config(String),
    /// Catch-all error variant for propagated anyhow errors.
    #[error(transparent)]
    Other(anyhow::Error),
}

impl GaalError {
    /// Returns the process exit code associated with this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NoResults => 1,
            Self::AmbiguousId(_) => 2,
            Self::NotFound(_) => 3,
            Self::NoIndex => 10,
            Self::ParseError(_) => 11,
            Self::Io(_) | Self::Db(_) | Self::Internal(_) | Self::Config(_) | Self::Other(_) => 1,
        }
    }

    fn fields(&self, command: &str) -> (String, String, String) {
        match self {
            Self::NoResults => no_results_message(command),
            Self::AmbiguousId(id) => (
                format!("Multiple sessions match `{id}`."),
                command_example(command),
                "Use a longer session ID prefix or inspect the available IDs with `gaal ls --since 30d -H`.".to_string(),
            ),
            Self::NotFound(target) => not_found_message(command, target),
            Self::NoIndex => (
                "The search index does not exist yet.".to_string(),
                "gaal index backfill".to_string(),
                "Build the index first, then rerun the command that depends on indexed session data.".to_string(),
            ),
            Self::ParseError(detail) => parse_error_message(command, detail),
            Self::Io(err) => (
                format!("A filesystem operation failed: {err}"),
                command_example(command),
                "Check that the referenced files and directories exist and that this machine has permission to read or write them.".to_string(),
            ),
            Self::Db(err) => (
                format!("The session database query failed: {err}"),
                command_example(command),
                "Try `gaal index backfill` if the database is stale, then rerun the command.".to_string(),
            ),
            Self::Internal(detail) => (
                format!("Gaal hit an internal error: {detail}"),
                command_example(command),
                "Retry once; if it repeats, capture this message and inspect the referenced session with `gaal inspect <session-id> -H`.".to_string(),
            ),
            Self::Config(detail) => (
                format!("Gaal configuration is invalid: {detail}"),
                command_example(command),
                "Fix the config value mentioned above, then rerun the command.".to_string(),
            ),
            Self::Other(err) => (
                format!("The command failed: {err}"),
                command_example(command),
                "Retry with `-H` for readable output or inspect the index and session inputs that this command depends on.".to_string(),
            ),
        }
    }

    /// Render an AX-compliant human-readable error for the active command.
    pub fn format_human(&self, command: &str) -> String {
        let (what, example, hint) = self.fields(command);

        format!("What went wrong: {what}\nExample: {example}\nHint: {hint}")
    }

    pub fn format_json(&self, command: &str) -> serde_json::Value {
        let (what, example, hint) = self.fields(command);
        serde_json::json!({
            "ok": false,
            "error": what,
            "hint": hint,
            "example": example,
            "exit_code": self.exit_code()
        })
    }
}

fn command_example(command: &str) -> String {
    match command {
        "ls" => "gaal ls --since 7d -H".to_string(),
        "inspect" => "gaal inspect 249aad1e -H".to_string(),
        "transcript" => "gaal transcript latest -H".to_string(),
        "who" => "gaal who ran cargo --since 7d -H".to_string(),
        "search" => "gaal search \"database migration\" -H".to_string(),
        "recall" => "gaal recall \"auth migration\" -H".to_string(),
        "find-salt" => "gaal find-salt GAAL_SALT_abc123".to_string(),
        "create-handoff" => "gaal create-handoff latest".to_string(),
        "index" => "gaal index backfill".to_string(),
        "tag" => "gaal tag 249aad1e deployment".to_string(),
        _ => "gaal ls --since 7d -H".to_string(),
    }
}

fn no_results_message(command: &str) -> (String, String, String) {
    match command {
        "ls" => (
            "No sessions matched those filters.".to_string(),
            "gaal ls --since 30d -H".to_string(),
            "Widen the time range, remove restrictive filters like `--tag`, or add `--all` to include short noise sessions.".to_string(),
        ),
        "search" => (
            "No indexed facts matched that search query.".to_string(),
            "gaal search \"build failure\" --since 30d -H".to_string(),
            "Try a broader term, a wider `--since` window, or a different `--field` filter.".to_string(),
        ),
        "who" => (
            "No sessions matched that attribution query.".to_string(),
            "gaal who ran cargo --since 30d -H".to_string(),
            "Broaden the time range, remove extra filters, or try another verb such as `read`, `wrote`, or `touched`.".to_string(),
        ),
        "inspect" => (
            "No sessions matched that inspect request.".to_string(),
            "gaal inspect latest -H".to_string(),
            "Try a specific session ID, `latest`, or list recent sessions first with `gaal ls --since 30d -H`.".to_string(),
        ),
        _ => (
            "The command completed but did not find any matching results.".to_string(),
            command_example(command),
            "Widen the search window or remove restrictive filters, then try again.".to_string(),
        ),
    }
}

fn not_found_message(command: &str, target: &str) -> (String, String, String) {
    match command {
        "transcript" => (
            format!("Session `{target}` was not found, so no transcript could be generated."),
            "gaal transcript latest -H".to_string(),
            "List recent sessions with `gaal ls --since 7d -H`, then rerun `gaal transcript` with a valid session ID.".to_string(),
        ),
        "inspect" => (
            format!("Session `{target}` was not found."),
            "gaal inspect latest -H".to_string(),
            "List recent sessions with `gaal ls --since 7d -H`, then rerun `gaal inspect` with a valid 8-character ID prefix.".to_string(),
        ),
        "find-salt" => (
            format!("No session JSONL file contains salt token `{target}`."),
            "gaal find-salt GAAL_SALT_abc123".to_string(),
            "Generate or copy the exact token with `gaal salt`, then rerun `gaal find-salt` with that full value.".to_string(),
        ),
        "tag" => (
            format!("Session `{target}` was not found, so tags could not be updated."),
            "gaal tag 249aad1e deployment".to_string(),
            "Find a valid session ID with `gaal ls --since 30d -H`, then rerun `gaal tag`.".to_string(),
        ),
        _ => (
            format!("`{target}` was not found."),
            command_example(command),
            "Check the identifier or path, then retry with a value that exists on this machine.".to_string(),
        ),
    }
}

fn parse_error_message(command: &str, detail: &str) -> (String, String, String) {
    match command {
        "search" if detail.contains("empty") || detail.contains("query") => (
            "The search query is empty.".to_string(),
            "gaal search \"database migration\" -H".to_string(),
            "Provide a non-empty query string, or use `gaal ls --since 30d -H` if you want to browse sessions instead of search fact text.".to_string(),
        ),
        "transcript" if detail.contains("session id") => (
            "The transcript command needs a session ID or `latest`.".to_string(),
            "gaal transcript latest -H".to_string(),
            "Pass a session ID prefix or `latest`, or run `gaal ls --since 7d -H` to find a session first.".to_string(),
        ),
        "inspect" if detail.contains("session id") || detail.contains("requires") => (
            "The inspect command needs a session selector.".to_string(),
            "gaal inspect latest -H".to_string(),
            "Pass a session ID, `latest`, `--ids`, or `--tag` to select at least one session.".to_string(),
        ),
        "who" if detail.contains("verb") => (
            "The `who` command needs a verb such as `read`, `wrote`, or `ran`.".to_string(),
            "gaal who ran cargo --since 7d -H".to_string(),
            "Choose one supported verb, then provide an optional target to narrow the match.".to_string(),
        ),
        "find-salt" if detail.contains("salt") => (
            "The `find-salt` command needs a salt token.".to_string(),
            "gaal find-salt GAAL_SALT_abc123".to_string(),
            "Generate a token with `gaal salt` or copy an existing one from a session, then rerun `gaal find-salt`.".to_string(),
        ),
        "tag" if detail.contains("session") => (
            "The `tag` command needs a session ID or `ls`.".to_string(),
            "gaal tag 249aad1e deployment".to_string(),
            "Use `gaal tag ls` to list known tags, or pass a session ID followed by one or more tags.".to_string(),
        ),
        "tag" if detail.contains("tag") => (
            format!("The tag command arguments are invalid: {detail}"),
            "gaal tag 249aad1e deployment".to_string(),
            "Pass at least one tag to add, or use `--remove` with one or more tags to delete.".to_string(),
        ),
        "index" => (
            format!("The index command arguments are invalid: {detail}"),
            "gaal index backfill".to_string(),
            "Check the subcommand flags and rerun with a valid date, path, or session ID.".to_string(),
        ),
        _ => (
            format!("The command arguments are invalid: {detail}"),
            command_example(command),
            "Adjust the arguments shown above, then rerun the command.".to_string(),
        ),
    }
}

impl From<rusqlite::Error> for GaalError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Db(value)
    }
}

impl From<std::io::Error> for GaalError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<anyhow::Error> for GaalError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}
