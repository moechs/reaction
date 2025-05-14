#![cfg(test)]

use std::{
    fs::File,
    io::Write,
    ops::Deref,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

pub struct Fixture {
    path: PathBuf,
    _tempdir: TempDir,
}

impl Fixture {
    // pub fn from_file(source: &Path) -> Self {
    //     let dir = TempDir::new().unwrap();
    //     let filename = source.file_name();
    //     let path = dir.path().join(&filename.unwrap());
    //     fs::copy(&source, &path).unwrap();
    //     Fixture {
    //         path,
    //         _tempdir: dir,
    //     }
    // }

    pub fn from_string(filename: &str, content: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(filename);
        let mut file = File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        Fixture {
            path,
            _tempdir: dir,
        }
    }

    pub fn empty(filename: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(filename);
        Fixture {
            _tempdir: dir,
            path,
        }
    }
}

impl Deref for Fixture {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path.deref()
    }
}

pub struct TempKeyspace {
    env: fjall::TransactionalKeyspace,
    _tempdir: TempDir,
}

impl Default for TempKeyspace {
    fn default() -> Self {
        let _tempdir = TempDir::new().unwrap();
        let env = fjall::Config::new(_tempdir.path()).open_transactional().unwrap();
        TempKeyspace { _tempdir, env }
    }
}

impl Deref for TempKeyspace {
    type Target = fjall::TransactionalKeyspace;

    fn deref(&self) -> &Self::Target {
        &self.env
    }
}
