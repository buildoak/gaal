pub mod discovery;
pub mod engine;
pub mod parent_parser;

pub use discovery::{collect_all_subagent_files, collect_subagent_files, SubagentFile};
pub use engine::{get_subagent_summaries, SubagentSummary};
pub use parent_parser::{extract_subagent_summaries, SubagentMeta};
