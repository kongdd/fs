//! Prevent retired SQLite indexes from being overwritten by updatedb.
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
};

pub fn reject_retired_index(path: &Path) -> Result<()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read database {}", path.display()));
        }
    };
    let mut header = Vec::new();
    file.take(16).read_to_end(&mut header)?;
    if header == b"SQLite format 3\0" {
        bail!(
            "{} is a retired SQLite/Rust index; configure a NEW database path and run fs updatedb update; the existing file will not be overwritten",
            path.display()
        );
    }
    // plocate itself validates other database headers and versions.
    Ok(())
}
