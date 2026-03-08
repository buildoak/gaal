//! Session-to-markdown rendering.
//!
//! Converts raw JSONL session files into human-readable markdown
//! preserving conversation flow. This is a parallel path to the
//! fact-extraction parser — it needs full conversation data
//! (content blocks, thinking blocks, tool results).

pub mod session_md;
