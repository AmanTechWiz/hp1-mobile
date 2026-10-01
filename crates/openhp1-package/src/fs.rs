//! Filesystem access shared by OpenHP1 loaders and executables.
//!
//! Native builds forward directly to `std::fs`. Browsers have no filesystem,
//! so web builds mount the player's imported installation and the OpenHP1
//! settings directory as a [`MemoryFs`] whose files are read on demand from
//! host storage. Callers use the same functions on every target.

mod memory;

pub use memory::{MemoryFs, Storage};

/// Where web builds mount the player's imported original installation.
pub const WEB_GAME_ROOT: &str = "/game";

/// Where web builds mount OpenHP1 settings, saves, and other user data.
pub const WEB_SETTINGS_DIR: &str = "/settings";

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::{
    DirEntry, FileType, Metadata, ReadDir, canonicalize, create_dir_all, metadata, read, read_dir,
    read_to_string, remove_file, rename, write,
};

#[cfg(target_arch = "wasm32")]
pub use memory::{DirEntry, FileType, Metadata, ReadDir};

/// Reads at most `len` bytes from the start of a file.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_prefix(path: impl AsRef<std::path::Path>, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    let mut bytes = Vec::with_capacity(len);
    std::fs::File::open(path)?
        .take(len as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(target_arch = "wasm32")]
pub use mounted::*;

#[cfg(target_arch = "wasm32")]
mod mounted {
    use std::{
        cell::RefCell,
        io,
        path::{Path, PathBuf},
    };

    use super::memory::{MemoryFs, Metadata, ReadDir};

    thread_local! {
        static MOUNTED: RefCell<MemoryFs> = RefCell::new(MemoryFs::empty());
    }

    /// Replaces the filesystem seen by every caller on this thread.
    pub fn mount(filesystem: MemoryFs) {
        MOUNTED.with_borrow_mut(|mounted| *mounted = filesystem);
    }

    fn with<T>(operation: impl FnOnce(&mut MemoryFs) -> T) -> T {
        MOUNTED.with_borrow_mut(operation)
    }

    pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        with(|fs| fs.read(path.as_ref()))
    }

    pub fn read_prefix(path: impl AsRef<Path>, len: usize) -> io::Result<Vec<u8>> {
        with(|fs| fs.read_prefix(path.as_ref(), len))
    }

    pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
        with(|fs| fs.read_to_string(path.as_ref()))
    }

    pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
        with(|fs| fs.read_dir(path.as_ref()))
    }

    pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
        with(|fs| fs.metadata(path.as_ref()))
    }

    pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
        with(|fs| fs.canonicalize(path.as_ref()))
    }

    pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
        with(|fs| fs.create_dir_all(path.as_ref()))
    }

    pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
        with(|fs| fs.write(path.as_ref(), contents.as_ref()))
    }

    pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
        with(|fs| fs.remove_file(path.as_ref()))
    }

    pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
        with(|fs| fs.rename(from.as_ref(), to.as_ref()))
    }
}
