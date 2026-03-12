# gaal: Fix token counting in `extract_claude_usage_event()`

Status: **ready to implement** (~15 min effort)

## Bug

`extract_claude_usage_event()` in `src/parser/claude.rs` (lines 178-191) only reads two fields:

```rust
fn extract_claude_usage_event(record: &Value) -> Option<EventKind> {
    let usage = record.pointer("/message/usage")?;
    if usage.is_null() {
        return None;
    }
    Some(EventKind::Usage {
        input_tokens: as_i64(record.pointer("/message/usage/input_tokens")),
        output_tokens: as_i64(record.pointer("/message/usage/output_tokens")),
        dedup_key: record
            .pointer("/message/id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}
```

This misses `cache_read_input_tokens` and `cache_creation_input_tokens`. The `input_tokens` field from the API only reports **non-cached** input tokens. In practice, most tokens hit the cache, so the reported `input_tokens` is a fraction of actual context usage.

## Reference: correct implementation

`src/commands/active.rs` lines 320-332 already handles this correctly:

```rust
let input = as_i64(record.pointer("/message/usage/input_tokens"));
let cache_read = as_i64(record.pointer("/message/usage/cache_read_input_tokens"));
let cache_creation = as_i64(record.pointer("/message/usage/cache_creation_input_tokens"));
let total_input = input + cache_read + cache_creation;
```

## Fix

1. Add `cache_read_input_tokens` and `cache_creation_input_tokens` fields to `EventKind::Usage` in `src/parser/event.rs`:

```rust
Usage {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    dedup_key: Option<String>,
},
```

2. Update `extract_claude_usage_event()` in `src/parser/claude.rs` to extract all three input fields and sum for total:

```rust
fn extract_claude_usage_event(record: &Value) -> Option<EventKind> {
    let usage = record.pointer("/message/usage")?;
    if usage.is_null() {
        return None;
    }
    let input = as_i64(record.pointer("/message/usage/input_tokens"));
    let cache_read = as_i64(record.pointer("/message/usage/cache_read_input_tokens"));
    let cache_creation = as_i64(record.pointer("/message/usage/cache_creation_input_tokens"));
    let total_input = input + cache_read + cache_creation;
    Some(EventKind::Usage {
        input_tokens: total_input,
        output_tokens: as_i64(record.pointer("/message/usage/output_tokens")),
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
        dedup_key: record
            .pointer("/message/id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}
```

3. Update all `EventKind::Usage { .. }` match arms across the codebase to handle the new fields.

4. Build and verify:

```bash
cargo build --release
```

The release binary is symlinked -- `cargo build --release` updates the target in place.
