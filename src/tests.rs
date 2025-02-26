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

pub struct TempDb {
    db: sled::Db,
    _tempdir: TempDir,
}

impl Default for TempDb {
    fn default() -> Self {
        let _tempdir = TempDir::new().unwrap();
        let db = sled::open(_tempdir.path()).unwrap();
        TempDb { _tempdir, db }
    }
}

impl Deref for TempDb {
    type Target = sled::Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
