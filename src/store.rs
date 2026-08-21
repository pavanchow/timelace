use crate::objects::{Object, ObjectId};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const REPO_DIR: &str = ".timelace";
const OBJECTS_DIR: &str = "objects";
const HEAD_FILE: &str = "HEAD";
const INDEX_FILE: &str = "index";

#[derive(Debug)]
pub enum StoreError {
    NotARepo(PathBuf),
    AlreadyARepo(PathBuf),
    Io(String),
    NotFound(String),
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotARepo(p) => {
                write!(f, "not a timelace repository (or any parent up to {})", p.display())
            }
            StoreError::AlreadyARepo(p) => {
                write!(f, "a timelace repository already exists at {}", p.display())
            }
            StoreError::Io(msg) => write!(f, "io error: {msg}"),
            StoreError::NotFound(what) => write!(f, "not found: {what}"),
            StoreError::Corrupt(what) => write!(f, "corrupt repository: {what}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A handle onto an on-disk `.timelace` repository rooted at `work_dir`.
pub struct Store {
    pub work_dir: PathBuf,
    pub repo_dir: PathBuf,
}

impl Store {
    /// Create a new repository at `work_dir`. Errors if one already exists.
    pub fn init(work_dir: &Path) -> Result<Store> {
        let repo_dir = work_dir.join(REPO_DIR);
        if repo_dir.exists() {
            return Err(StoreError::AlreadyARepo(work_dir.to_path_buf()));
        }
        fs::create_dir_all(repo_dir.join(OBJECTS_DIR))?;
        fs::write(repo_dir.join(HEAD_FILE), "")?;
        fs::write(repo_dir.join(INDEX_FILE), "")?;
        Ok(Store {
            work_dir: work_dir.to_path_buf(),
            repo_dir,
        })
    }

    /// Locate an existing repository by walking up from `start` looking for `.timelace`.
    pub fn discover(start: &Path) -> Result<Store> {
        let mut dir = start
            .canonicalize()
            .map_err(|_| StoreError::NotARepo(start.to_path_buf()))?;
        loop {
            let candidate = dir.join(REPO_DIR);
            if candidate.is_dir() {
                return Ok(Store {
                    work_dir: dir,
                    repo_dir: candidate,
                });
            }
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => return Err(StoreError::NotARepo(start.to_path_buf())),
            }
        }
    }

    fn object_path(&self, id: &ObjectId) -> PathBuf {
        let (dir, rest) = id.as_str().split_at(2);
        self.repo_dir.join(OBJECTS_DIR).join(dir).join(rest)
    }

    /// Write an object to the store, deduping automatically since the path is
    /// derived from the content hash: writing the same bytes twice is a no-op.
    pub fn write_object(&self, obj: &Object) -> Result<ObjectId> {
        let id = obj.id();
        let path = self.object_path(&id);
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(&path, obj.encode())?;
        }
        Ok(id)
    }

    pub fn read_object(&self, id: &ObjectId) -> Result<Object> {
        let path = self.object_path(id);
        let bytes = fs::read(&path)
            .map_err(|_| StoreError::NotFound(format!("object {id}")))?;
        Object::decode(&bytes).map_err(StoreError::Corrupt)
    }

    pub fn has_object(&self, id: &ObjectId) -> bool {
        self.object_path(id).exists()
    }

    pub fn read_head(&self) -> Result<Option<ObjectId>> {
        let text = fs::read_to_string(self.repo_dir.join(HEAD_FILE))?;
        let text = text.trim();
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ObjectId::parse(text).map_err(StoreError::Corrupt)?))
        }
    }

    pub fn write_head(&self, id: &ObjectId) -> Result<()> {
        fs::write(self.repo_dir.join(HEAD_FILE), id.as_str())?;
        Ok(())
    }

    /// The staging list: relative paths (as recorded by `add`) that will be
    /// included in the next commit's tree snapshot.
    pub fn read_index(&self) -> Result<Vec<String>> {
        let text = fs::read_to_string(self.repo_dir.join(INDEX_FILE))?;
        Ok(text.lines().map(|s| s.to_string()).filter(|s| !s.is_empty()).collect())
    }

    pub fn write_index(&self, paths: &[String]) -> Result<()> {
        let mut sorted = paths.to_vec();
        sorted.sort();
        sorted.dedup();
        fs::write(self.repo_dir.join(INDEX_FILE), sorted.join("\n"))?;
        Ok(())
    }

    pub fn add_to_index(&self, rel_path: &str) -> Result<()> {
        let mut paths = self.read_index()?;
        paths.push(rel_path.to_string());
        self.write_index(&paths)
    }
}
