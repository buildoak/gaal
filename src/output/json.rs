use std::io::{self, Write};

use anyhow::Result;
use serde::Serialize;

/// Print any serializable value as pretty JSON to stdout.
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer_pretty(&mut lock, value)?;
    writeln!(&mut lock)?;
    Ok(())
}
