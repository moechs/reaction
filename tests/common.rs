#![cfg(unix)]
#![allow(dead_code)]

use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

/// Make config files copied into a temp dir satisfy reaction's 0600 check.
/// Executables (plugin binaries) are left untouched.
pub fn restrict_configs(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_file() && meta.mode() & 0o111 == 0 {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

/// Copy a config directory into `dest` with private permissions.
pub fn copy_config_tree(from: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    fs::set_permissions(dest, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        let meta = entry.metadata()?;
        if meta.is_dir() {
            copy_config_tree(&entry.path(), &target)?;
        } else if meta.is_file() {
            fs::copy(entry.path(), &target)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

/// Copy a single config file into `dest` with private permissions.
pub fn copy_private_config(from: &Path, dest: &Path) -> io::Result<()> {
    fs::copy(from, dest)?;
    fs::set_permissions(dest, fs::Permissions::from_mode(0o600))
}

/// Set a config file written directly by a test to 0600.
pub fn restrict_file(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}
