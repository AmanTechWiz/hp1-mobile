use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, io,
    ops::Bound,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Host storage behind a [`MemoryFs`].
pub trait Storage {
    /// Reads a stored file, or only its first `limit` bytes.
    fn read(&self, path: &Path, limit: Option<usize>) -> io::Result<Vec<u8>>;

    /// Persists a written file's complete contents, or forgets a removed file.
    fn persist(&self, path: &Path, contents: Option<&[u8]>);
}

enum Node {
    Directory,
    Stored { len: u64 },
    Written(Arc<[u8]>),
}

/// A rooted file tree whose stored files stay in host storage until read.
///
/// The browser has no filesystem, and an original installation is too large
/// to keep resident on mobile devices. Files listed with [`MemoryFs::add_stored`]
/// are read through [`Storage`] on every access; written files are kept in
/// memory and forwarded to [`Storage::persist`].
pub struct MemoryFs {
    nodes: BTreeMap<PathBuf, Node>,
    storage: Option<Box<dyn Storage>>,
}

impl MemoryFs {
    pub fn new(storage: Box<dyn Storage>) -> Self {
        Self {
            storage: Some(storage),
            ..Self::empty()
        }
    }

    pub(crate) fn empty() -> Self {
        Self {
            nodes: BTreeMap::from([(PathBuf::from("/"), Node::Directory)]),
            storage: None,
        }
    }

    /// Lists a file of `len` bytes that [`Storage::read`] can provide.
    pub fn add_stored(&mut self, path: impl AsRef<Path>, len: u64) -> io::Result<()> {
        let path = normalize(path.as_ref())?;
        let parent = path.parent().ok_or_else(|| is_a_directory(&path))?;
        self.create_dir_all(parent)?;
        if matches!(self.nodes.get(&path), Some(Node::Directory)) {
            return Err(is_a_directory(&path));
        }
        self.nodes.insert(path, Node::Stored { len });
        Ok(())
    }

    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let path = normalize(path)?;
        match self.node(&path)? {
            Node::Directory => Err(is_a_directory(&path)),
            Node::Stored { .. } => self.storage(&path)?.read(&path, None),
            Node::Written(contents) => Ok(contents.to_vec()),
        }
    }

    pub fn read_prefix(&self, path: &Path, len: usize) -> io::Result<Vec<u8>> {
        let path = normalize(path)?;
        match self.node(&path)? {
            Node::Directory => Err(is_a_directory(&path)),
            Node::Stored { .. } => {
                let mut bytes = self.storage(&path)?.read(&path, Some(len))?;
                bytes.truncate(len);
                Ok(bytes)
            }
            Node::Written(contents) => Ok(contents[..len.min(contents.len())].to_vec()),
        }
    }

    pub fn read_to_string(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn read_dir(&self, path: &Path) -> io::Result<ReadDir> {
        let path = normalize(path)?;
        if !matches!(self.node(&path)?, Node::Directory) {
            return Err(not_a_directory(&path));
        }
        let entries = self
            .nodes
            .range::<Path, _>((Bound::Excluded(path.as_path()), Bound::Unbounded))
            .take_while(|(candidate, _)| candidate.starts_with(&path))
            .filter(|(candidate, _)| candidate.parent() == Some(path.as_path()))
            .map(|(candidate, node)| DirEntry {
                path: candidate.clone(),
                metadata: Metadata::of(node),
            })
            .collect::<Vec<_>>();
        Ok(ReadDir(entries.into_iter()))
    }

    pub fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let path = normalize(path)?;
        self.node(&path).map(Metadata::of)
    }

    pub fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let path = normalize(path)?;
        self.node(&path)?;
        Ok(path)
    }

    pub fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        let path = normalize(path)?;
        for ancestor in path.ancestors() {
            match self.nodes.get(ancestor) {
                Some(Node::Directory) => break,
                Some(_) => return Err(not_a_directory(ancestor)),
                None => {}
            }
        }
        for ancestor in path.ancestors() {
            self.nodes
                .entry(ancestor.to_path_buf())
                .or_insert(Node::Directory);
        }
        Ok(())
    }

    pub fn write(&mut self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let path = normalize(path)?;
        let parent = path.parent().ok_or_else(|| is_a_directory(&path))?;
        if !matches!(self.node(parent)?, Node::Directory) {
            return Err(not_a_directory(parent));
        }
        if matches!(self.nodes.get(&path), Some(Node::Directory)) {
            return Err(is_a_directory(&path));
        }
        if let Some(storage) = &self.storage {
            storage.persist(&path, Some(contents));
        }
        self.nodes.insert(path, Node::Written(Arc::from(contents)));
        Ok(())
    }

    pub fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        let path = normalize(path)?;
        if matches!(self.node(&path)?, Node::Directory) {
            return Err(is_a_directory(&path));
        }
        self.nodes.remove(&path);
        if let Some(storage) = &self.storage {
            storage.persist(&path, None);
        }
        Ok(())
    }

    pub fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let contents = self.read(from)?;
        self.write(to, &contents)?;
        if normalize(from)? != normalize(to)? {
            self.remove_file(from)?;
        }
        Ok(())
    }

    fn node(&self, path: &Path) -> io::Result<&Node> {
        self.nodes.get(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("`{}` does not exist", path.display()),
            )
        })
    }

    fn storage(&self, path: &Path) -> io::Result<&dyn Storage> {
        self.storage.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!("`{}` has no mounted storage", path.display()),
            )
        })
    }
}

impl fmt::Debug for MemoryFs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryFs")
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

/// Resolves `.` and `..` lexically; relative paths start at the root.
fn normalize(path: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(name) => normalized.push(name),
            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{}` has a platform prefix", path.display()),
                ));
            }
        }
    }
    Ok(normalized)
}

fn is_a_directory(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::IsADirectory,
        format!("`{}` is a directory", path.display()),
    )
}

fn not_a_directory(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotADirectory,
        format!("`{}` is not a directory", path.display()),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileType {
    directory: bool,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.directory
    }

    pub fn is_file(&self) -> bool {
        !self.directory
    }

    pub fn is_symlink(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    file_type: FileType,
    len: u64,
}

impl Metadata {
    fn of(node: &Node) -> Self {
        let (directory, len) = match node {
            Node::Directory => (true, 0),
            Node::Stored { len } => (false, *len),
            Node::Written(contents) => (false, contents.len() as u64),
        };
        Self {
            file_type: FileType { directory },
            len,
        }
    }

    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

#[derive(Debug)]
pub struct DirEntry {
    path: PathBuf,
    metadata: Metadata,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn file_name(&self) -> OsString {
        self.path.file_name().unwrap_or_default().to_owned()
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.metadata.file_type)
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(self.metadata.clone())
    }
}

#[derive(Debug)]
pub struct ReadDir(std::vec::IntoIter<DirEntry>);

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use super::*;

    type Persisted = Rc<RefCell<Vec<(PathBuf, Option<Vec<u8>>)>>>;

    struct FakeStorage {
        files: HashMap<PathBuf, Vec<u8>>,
        persisted: Persisted,
    }

    impl Storage for FakeStorage {
        fn read(&self, path: &Path, limit: Option<usize>) -> io::Result<Vec<u8>> {
            let bytes = self
                .files
                .get(path)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            Ok(bytes[..limit.unwrap_or(bytes.len()).min(bytes.len())].to_vec())
        }

        fn persist(&self, path: &Path, contents: Option<&[u8]>) {
            self.persisted
                .borrow_mut()
                .push((path.to_path_buf(), contents.map(<[u8]>::to_vec)));
        }
    }

    fn filesystem() -> (MemoryFs, Persisted) {
        let persisted = Persisted::default();
        let storage = FakeStorage {
            files: HashMap::from([
                (
                    PathBuf::from("/game/System/Engine.u"),
                    b"\xc1\x83\x2a\x9eEngine".to_vec(),
                ),
                (
                    PathBuf::from("/game/System/Default.ini"),
                    b"[URL]\n".to_vec(),
                ),
            ]),
            persisted: Rc::clone(&persisted),
        };
        let mut fs = MemoryFs::new(Box::new(storage));
        fs.add_stored("/game/System/Engine.u", 10).unwrap();
        fs.add_stored("/game/System/Default.ini", 6).unwrap();
        fs.add_stored("/game/Maps/Lev_Tut1.unr", 4).unwrap();
        (fs, persisted)
    }

    #[test]
    fn stored_files_are_read_whole_or_by_prefix_through_storage() {
        let (fs, _) = filesystem();
        assert_eq!(
            fs.read(Path::new("/game/System/Engine.u")).unwrap(),
            b"\xc1\x83\x2a\x9eEngine"
        );
        assert_eq!(
            fs.read_prefix(Path::new("/game/System/Engine.u"), 4)
                .unwrap(),
            0x9e2a_83c1_u32.to_le_bytes()
        );
        assert_eq!(
            fs.read_to_string(Path::new("/game/./Maps/../System/Default.ini"))
                .unwrap(),
            "[URL]\n"
        );
        assert_eq!(
            fs.metadata(Path::new("/game/Maps/Lev_Tut1.unr"))
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn directories_are_implied_and_list_only_direct_children() {
        let (fs, _) = filesystem();
        let names = |path: &str| {
            fs.read_dir(Path::new(path))
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (
                        entry.file_name().into_string().unwrap(),
                        entry.file_type().unwrap().is_dir(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(names("/"), [("game".to_owned(), true)]);
        assert_eq!(
            names("/game"),
            [("Maps".to_owned(), true), ("System".to_owned(), true)]
        );
        assert_eq!(
            names("/game/System"),
            [
                ("Default.ini".to_owned(), false),
                ("Engine.u".to_owned(), false)
            ]
        );
        assert!(fs.metadata(Path::new("/game/System")).unwrap().is_dir());
        assert_eq!(
            fs.read_dir(Path::new("/game/System/Engine.u"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotADirectory
        );
    }

    #[test]
    fn missing_paths_report_not_found() {
        let (fs, _) = filesystem();
        for error in [
            fs.read(Path::new("/game/System/Missing.u")).unwrap_err(),
            fs.canonicalize(Path::new("/game/Missing")).unwrap_err(),
            fs.read_dir(Path::new("/settings")).unwrap_err(),
        ] {
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
        }
        assert_eq!(
            fs.canonicalize(Path::new("/game/Maps/../Maps/Lev_Tut1.unr"))
                .unwrap(),
            Path::new("/game/Maps/Lev_Tut1.unr")
        );
    }

    #[test]
    fn writes_stay_resident_and_are_persisted() {
        let (mut fs, persisted) = filesystem();
        let save = Path::new("/settings/Saves/save1.usa");
        assert_eq!(
            fs.write(save, b"one").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs.create_dir_all(Path::new("/settings/Saves")).unwrap();
        fs.write(save, b"one").unwrap();
        assert_eq!(fs.read(save).unwrap(), b"one");
        assert_eq!(fs.read_prefix(save, 16).unwrap(), b"one");

        let temporary = Path::new("/settings/Saves/.save2.tmp");
        fs.write(temporary, b"two").unwrap();
        fs.rename(temporary, save).unwrap();
        assert_eq!(fs.read(save).unwrap(), b"two");
        assert!(fs.metadata(temporary).is_err());
        fs.remove_file(save).unwrap();
        assert!(fs.metadata(save).is_err());

        assert_eq!(
            *persisted.borrow(),
            [
                (save.to_path_buf(), Some(b"one".to_vec())),
                (temporary.to_path_buf(), Some(b"two".to_vec())),
                (save.to_path_buf(), Some(b"two".to_vec())),
                (temporary.to_path_buf(), None),
                (save.to_path_buf(), None),
            ]
        );
    }

    #[test]
    fn files_and_directories_do_not_replace_each_other() {
        let (mut fs, _) = filesystem();
        assert_eq!(
            fs.create_dir_all(Path::new("/game/System/Engine.u/Inner"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(
            fs.write(Path::new("/game/System"), b"").unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
        assert_eq!(
            fs.add_stored("/game/Maps", 1).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
    }
}
