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

pub struct TempEnv {
    env: heed::Env,
    _tempdir: TempDir,
}

impl Default for TempEnv {
    fn default() -> Self {
        let _tempdir = TempDir::new().unwrap();
        #[allow(unsafe_code)]
        let env = unsafe {
            heed::EnvOpenOptions::new()
                .max_dbs(20)
                .open(_tempdir.path())
        }
        .unwrap();
        TempEnv { _tempdir, env }
    }
}

impl Deref for TempEnv {
    type Target = heed::Env;

    fn deref(&self) -> &Self::Target {
        &self.env
    }
}
