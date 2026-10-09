//! A temporary directory for tests, shared by the unit tests in `src` and the
//! integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// A temporary directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates an empty directory. `name` must be unique within the test binary.
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("caffeinate-test-{name}-{}", std::process::id()));
        // A killed test run may have left a directory with this name behind.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Creates a file in the directory and returns its path.
    pub fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
