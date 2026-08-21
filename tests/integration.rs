use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use timelace::commands;
use timelace::store::Store;
use timelace::Object;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp directory under std::env::temp_dir(), unique per test, cleaned up on drop.
/// Avoids pulling in a tempfile dependency for something this small.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("timelace-test-{label}-{nanos}-{n}"));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn init_creates_store() {
    let dir = TempDir::new("init");
    commands::init(dir.path()).unwrap();
    assert!(dir.path().join(".timelace").is_dir());
    assert!(dir.path().join(".timelace/objects").is_dir());
    assert!(dir.path().join(".timelace/HEAD").is_file());
}

#[test]
fn add_commit_stores_objects_and_advances_head() {
    let dir = TempDir::new("add-commit");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();

    fs::write(dir.path().join("hello.txt"), b"hello timelace").unwrap();
    commands::add(&store, Path::new("hello.txt")).unwrap();

    assert!(store.read_head().unwrap().is_none());
    let commit_id = commands::commit(&store, "first commit").unwrap();

    let head = store.read_head().unwrap();
    assert_eq!(head.as_ref(), Some(&commit_id));

    let obj = store.read_object(&commit_id).unwrap();
    match obj {
        Object::Commit { parent, message, .. } => {
            assert!(parent.is_none());
            assert_eq!(message, "first commit");
        }
        _ => panic!("expected a commit object"),
    }
}

#[test]
fn second_commit_records_first_as_parent() {
    let dir = TempDir::new("parent-chain");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();

    fs::write(dir.path().join("a.txt"), b"version one").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    let first = commands::commit(&store, "commit one").unwrap();

    fs::write(dir.path().join("a.txt"), b"version two").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    let second = commands::commit(&store, "commit two").unwrap();

    let obj = store.read_object(&second).unwrap();
    match obj {
        Object::Commit { parent, .. } => assert_eq!(parent, Some(first.clone())),
        _ => panic!("expected a commit object"),
    }

    let head = store.read_head().unwrap();
    assert_eq!(head, Some(second));
}

#[test]
fn log_shows_both_commits_in_order() {
    let dir = TempDir::new("log");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();

    fs::write(dir.path().join("a.txt"), b"one").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    let first = commands::commit(&store, "commit one").unwrap();

    fs::write(dir.path().join("a.txt"), b"two").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    let second = commands::commit(&store, "commit two").unwrap();

    let entries = commands::log(&store).unwrap();
    let ids: Vec<_> = entries.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(ids, vec![second, first]);

    let messages: Vec<_> = entries
        .iter()
        .map(|(_, obj)| match obj {
            Object::Commit { message, .. } => message.clone(),
            _ => panic!("expected commit"),
        })
        .collect();
    assert_eq!(messages, vec!["commit two", "commit one"]);
}

#[test]
fn identical_content_shares_one_blob_id() {
    let dir = TempDir::new("dedupe");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();

    fs::write(dir.path().join("a.txt"), b"same bytes").unwrap();
    fs::write(dir.path().join("b.txt"), b"same bytes").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    commands::add(&store, Path::new("b.txt")).unwrap();
    let commit_id = commands::commit(&store, "dedupe test").unwrap();

    let commit_obj = store.read_object(&commit_id).unwrap();
    let tree_id = match commit_obj {
        Object::Commit { tree, .. } => tree,
        _ => panic!("expected commit"),
    };
    let tree_obj = store.read_object(&tree_id).unwrap();
    let entries = match tree_obj {
        Object::Tree(entries) => entries,
        _ => panic!("expected tree"),
    };
    assert_eq!(entries.len(), 2);
    let ids: Vec<_> = entries.iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids[0], ids[1], "identical content must dedupe to one object id");

    // And only one file exists on disk in the object store for that id.
    let object_dir = dir
        .path()
        .join(".timelace/objects")
        .join(&ids[0].as_str()[..2]);
    let count = fs::read_dir(&object_dir).unwrap().count();
    assert_eq!(count, 1);
}

#[test]
fn checkout_restores_earlier_commit_exactly() {
    let dir = TempDir::new("checkout");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();

    fs::write(dir.path().join("a.txt"), b"first version").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    let first = commands::commit(&store, "commit one").unwrap();

    fs::write(dir.path().join("a.txt"), b"second version").unwrap();
    fs::write(dir.path().join("b.txt"), b"only in second commit").unwrap();
    commands::add(&store, Path::new("a.txt")).unwrap();
    commands::add(&store, Path::new("b.txt")).unwrap();
    commands::commit(&store, "commit two").unwrap();

    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"second version"
    );
    assert!(dir.path().join("b.txt").exists());

    commands::checkout(&store, &first).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "first version"
    );
    assert!(
        !dir.path().join("b.txt").exists(),
        "b.txt should not exist after checking out the commit that predates it"
    );
    assert_eq!(store.read_head().unwrap(), Some(first));
}

#[test]
fn operating_outside_a_repo_returns_a_clear_error() {
    let dir = TempDir::new("no-repo");
    let result = Store::discover(dir.path());
    assert!(result.is_err());
}

#[test]
fn bad_commit_id_on_checkout_returns_a_clear_error() {
    let dir = TempDir::new("bad-checkout");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();
    let bad_id = timelace::ObjectId::parse(&"0".repeat(64)).unwrap();
    let result = commands::checkout(&store, &bad_id);
    assert!(result.is_err());
}

#[test]
fn commit_without_staged_files_returns_a_clear_error() {
    let dir = TempDir::new("empty-commit");
    commands::init(dir.path()).unwrap();
    let store = Store::discover(dir.path()).unwrap();
    let result = commands::commit(&store, "nothing to see here");
    assert!(result.is_err());
}
