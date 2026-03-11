use std::fmt::Write;

use crate::error::GaalError;

/// Generate and print a random session salt token.
pub fn run() -> Result<(), GaalError> {
    let bytes = rand::random::<[u8; 8]>();
    let mut hex = String::with_capacity(16);

    for byte in bytes {
        write!(&mut hex, "{byte:02x}")
            .map_err(|err| GaalError::Internal(format!("failed to format salt bytes: {err}")))?;
    }

    println!("GAAL_SALT_{hex}");
    Ok(())
}
