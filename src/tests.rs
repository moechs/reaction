#![cfg(test)]

use std::{
    fs::File,
    io::Write,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use treedb::{Database, LoadedDB};

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

pub struct TempDatabase {
    db: Database,
    _tempdir: TempDir,
}

impl TempDatabase {
    pub async fn default() -> Self {
        let _tempdir = TempDir::new().unwrap();
        let db = Database::from_dir(_tempdir.path().to_owned(), None)
            .await
            .unwrap();
        TempDatabase { _tempdir, db }
    }

    pub async fn from_loaded_db(loaded_db: LoadedDB) -> Self {
        let _tempdir = TempDir::new().unwrap();
        let db = Database::from_dir(_tempdir.path().to_owned(), Some(loaded_db))
            .await
            .unwrap();
        TempDatabase { _tempdir, db }
    }
}

impl Deref for TempDatabase {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl DerefMut for TempDatabase {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}
