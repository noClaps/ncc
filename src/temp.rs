//! Private, uniquely created build directories; no external runtime dependency.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Directory(PathBuf);
impl Directory {
    /// Create a private directory in the system temporary directory.
    ///
    /// # Errors
    /// Returns an I/O error if directory creation fails or all 100 candidate names
    /// already exist.
    pub fn new() -> io::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }
    /// Create a private directory under `parent`.
    ///
    /// # Errors
    /// Returns an I/O error if directory creation fails (for example, if the parent
    /// is missing or not writable), or all 100 candidate names already exist.
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..100 {
            let path = parent.join(format!(
                "ncc-{}-{nonce:x}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate unique temporary directory",
        ))
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
