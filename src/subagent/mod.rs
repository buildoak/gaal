pub mod discovery;
pub mod parent_parser;
pub mod engine;

pub use discovery::{collect_all_subagent_files, collect_subagent_files, SubagentFile};
pub use engine::{get_subagent_summaries, SubagentSummary};
pub use parent_parser::{extract_subagent_summaries, SubagentMeta};
