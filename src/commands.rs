use crate::objects::{Object, ObjectId, TreeEntry};
use crate::store::{Result, Store, StoreError};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn init(work_dir: &Path) -> Result<()> {
    Store::init(work_dir)?;
    println!("initialized empty timelace repository in {}", work_dir.join(".timelace").display());
    Ok(())
}

pub fn add(store: &Store, path: &Path) -> Result<()> {
    let abs = store.work_dir.join(path);
    if !abs.exists() {
        return Err(StoreError::NotFound(format!("path {}", path.display())));
    }
    if abs.is_dir() {
        for entry in walk_files(&abs)? {
            let rel = entry
                .strip_prefix(&store.work_dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            store.add_to_index(&rel)?;
        }
    } else {
        let rel = abs
            .strip_prefix(&store.work_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        store.add_to_index(&rel)?;
    }
    Ok(())
}

fn walk_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".timelace").unwrap_or(false) {
            continue;
        }
        if path.is_dir() {
            out.extend(walk_files(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

/// Build the tree object graph for a set of staged relative paths, writing
/// blobs and intermediate trees into the store, and return the root tree id.
fn build_tree(store: &Store, paths: &[String]) -> Result<ObjectId> {
    // Nest paths into a directory structure keyed by path segment.
    #[derive(Default)]
    struct Node {
        file: Option<String>, // full relative path, for leaf files
        children: BTreeMap<String, Node>,
    }

    let mut root = Node::default();
    for p in paths {
        let mut node = &mut root;
        let segments: Vec<&str> = p.split('/').collect();
        for (i, seg) in segments.iter().enumerate() {
            if i == segments.len() - 1 {
                node = node.children.entry(seg.to_string()).or_default();
                node.file = Some(p.clone());
            } else {
                node = node.children.entry(seg.to_string()).or_default();
            }
        }
    }

    fn write_node(store: &Store, node: &Node) -> Result<ObjectId> {
        let mut entries = Vec::new();
        for (name, child) in &node.children {
            if let Some(rel) = &child.file {
                let bytes = fs::read(store.work_dir.join(rel))?;
                let blob = Object::Blob(bytes);
                let id = store.write_object(&blob)?;
                entries.push(TreeEntry {
                    name: name.clone(),
                    is_tree: false,
                    id,
                });
            } else {
                let id = write_node(store, child)?;
                entries.push(TreeEntry {
                    name: name.clone(),
                    is_tree: true,
                    id,
                });
            }
        }
        let tree = Object::Tree(entries);
        store.write_object(&tree)
    }

    write_node(store, &root)
}

pub fn commit(store: &Store, message: &str) -> Result<ObjectId> {
    let paths = store.read_index()?;
    if paths.is_empty() {
        return Err(StoreError::NotFound("nothing staged, run 'add' first".into()));
    }
    let tree = build_tree(store, &paths)?;
    let parent = store.read_head()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let commit_obj = Object::Commit {
        tree,
        parent,
        message: message.to_string(),
        timestamp,
    };
    let id = store.write_object(&commit_obj)?;
    store.write_head(&id)?;
    Ok(id)
}

pub fn log(store: &Store) -> Result<Vec<(ObjectId, Object)>> {
    let mut out = Vec::new();
    let mut current = store.read_head()?;
    while let Some(id) = current {
        let obj = store.read_object(&id)?;
        let parent = match &obj {
            Object::Commit { parent, .. } => parent.clone(),
            _ => return Err(StoreError::Corrupt(format!("{id} is not a commit"))),
        };
        out.push((id, obj));
        current = parent;
    }
    Ok(out)
}

pub enum StatusEntry {
    Staged(String),
    Untracked(String),
}

/// A basic comparison: files on disk (outside .timelace) vs. what is staged.
/// Staged-but-missing-on-disk and present-but-unstaged are both reported.
pub fn status(store: &Store) -> Result<Vec<StatusEntry>> {
    let staged = store.read_index()?;
    let staged_set: std::collections::BTreeSet<_> = staged.iter().cloned().collect();
    let mut out = Vec::new();
    for p in &staged {
        out.push(StatusEntry::Staged(p.clone()));
    }
    for path in walk_files(&store.work_dir)? {
        let rel = path
            .strip_prefix(&store.work_dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !staged_set.contains(&rel) {
            out.push(StatusEntry::Untracked(rel));
        }
    }
    Ok(out)
}

/// Restore the working tree to exactly match the snapshot recorded by `commit_id`.
/// Files not present in that commit's tree are removed; files present are
/// overwritten with the stored blob content.
pub fn checkout(store: &Store, commit_id: &ObjectId) -> Result<()> {
    let obj = store
        .read_object(commit_id)
        .map_err(|_| StoreError::NotFound(format!("commit {commit_id}")))?;
    let tree_id = match obj {
        Object::Commit { tree, .. } => tree,
        _ => return Err(StoreError::Corrupt(format!("{commit_id} is not a commit"))),
    };

    // Wipe existing tracked files (everything outside .timelace) before restoring.
    for path in walk_files(&store.work_dir)? {
        fs::remove_file(&path)?;
    }
    remove_empty_dirs(&store.work_dir)?;

    restore_tree(store, &tree_id, &store.work_dir)?;
    store.write_head(commit_id)?;
    Ok(())
}

fn restore_tree(store: &Store, tree_id: &ObjectId, dest: &Path) -> Result<()> {
    let obj = store.read_object(tree_id)?;
    let entries = match obj {
        Object::Tree(entries) => entries,
        _ => return Err(StoreError::Corrupt(format!("{tree_id} is not a tree"))),
    };
    for entry in entries {
        let path = dest.join(&entry.name);
        if entry.is_tree {
            fs::create_dir_all(&path)?;
            restore_tree(store, &entry.id, &path)?;
        } else {
            let obj = store.read_object(&entry.id)?;
            let bytes = match obj {
                Object::Blob(b) => b,
                _ => return Err(StoreError::Corrupt(format!("{} is not a blob", entry.id))),
            };
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, bytes)?;
        }
    }
    Ok(())
}

fn remove_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().map(|n| n == ".timelace").unwrap_or(false) {
            continue;
        }
        if path.is_dir() {
            remove_empty_dirs(&path)?;
            let _ = fs::remove_dir(&path);
        }
    }
    Ok(())
}
