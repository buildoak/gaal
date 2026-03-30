use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::discovery::{collect_subagent_files, SubagentFile};
use super::parent_parser::{
    extract_codex_spawn_summaries, extract_subagent_summaries, SubagentMeta,
};

#[derive(Debug, Clone)]
pub struct SubagentSummary {
    pub meta: SubagentMeta,
    pub jsonl_path: Option<PathBuf>,
    pub has_jsonl: bool,
}

pub fn get_subagent_summaries(
    parent_jsonl: &Path,
    session_dir: &Path,
) -> Result<Vec<SubagentSummary>> {
    let parent_path = parent_jsonl.to_string_lossy();
    let is_codex_parent = parent_path.contains("/.codex/");
    let metas = if is_codex_parent {
        extract_codex_spawn_summaries(parent_jsonl)?
    } else {
        extract_subagent_summaries(parent_jsonl)?
    };
    if metas.is_empty() {
        return Ok(Vec::new());
    }

    if is_codex_parent {
        return Ok(metas
            .into_iter()
            .map(|meta| SubagentSummary {
                meta,
                jsonl_path: None,
                has_jsonl: false,
            })
            .collect());
    }

    let discovered = collect_subagent_files(session_dir);
    let mut file_map: HashMap<String, SubagentFile> = HashMap::new();
    for file in discovered {
        file_map.insert(file.agent_id.clone(), file);
    }

    let mut summaries = Vec::new();
    for meta in metas {
        let meta_stripped = meta
            .agent_id
            .strip_prefix("agent-")
            .unwrap_or(&meta.agent_id);
        let matched_file = file_map
            .iter()
            .find(|(file_prefix, _)| {
                meta_stripped.starts_with(file_prefix.as_str())
                    || file_prefix.starts_with(meta_stripped)
            })
            .map(|(_, f)| f);

        let (jsonl_path, has_jsonl) = match matched_file {
            Some(f) => (Some(f.path.clone()), true),
            None => (None, false),
        };

        summaries.push(SubagentSummary {
            meta,
            jsonl_path,
            has_jsonl,
        });
    }

    Ok(summaries)
}
