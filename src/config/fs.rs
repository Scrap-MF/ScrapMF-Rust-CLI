//! Low-level config file operations: atomic writes and permissions.
use std::path::Path;

use anyhow::Context;

/// Atomically write `content` to `path`: temp file in the same directory +
/// `rename`. Prevents half-written TOML files if the process crashes mid-write.
pub(super) fn write_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, content).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("atomic rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Persist a config file safely: atomic write and `0o600` permissions
/// (config files may hold cookie paths).
pub(crate) fn write_config_file(path: &Path, content: &str) -> anyhow::Result<()> {
    write_atomic(path, content).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Restrict a directory/file to owner-only access (0o700 dirs, 0o600 files).
/// Best-effort: failures are ignored (best possible on non-unix).
pub(crate) fn restrict_perms(path: &Path, dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if dir { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod fs_safety_tests {
    use super::write_atomic;
    use tempfile::TempDir;

    #[test]
    fn write_atomic_writes_content() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_atomic(&path, "a = 1").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "a = 1");
        // No temp residue left behind
        let residue: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "temp files left: {residue:?}");
    }

    #[test]
    fn write_atomic_replaces_existing_content() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_atomic(&path, "old").expect("write old");
        write_atomic(&path, "new").expect("write new");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
    }
}
