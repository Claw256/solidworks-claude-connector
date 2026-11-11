use std::path::PathBuf;

/// Default store location: `%LOCALAPPDATA%\MTD CAD\memory.db` (or `$HOME/.mtd-cad/memory.db`).
/// Override with `MTD_DB`.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("MTD_DB") {
        return PathBuf::from(p);
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(l).join("MTD CAD").join("memory.db");
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mtd-cad").join("memory.db")
}
