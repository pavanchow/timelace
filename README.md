# Timelace

**A tiny, readable version control system in Rust, built from scratch.**

Git works the same way underneath: content-addressed blobs, trees, and commits linked by SHA-256 hashes. But git's implementation is enormous, and the object model gets buried under decades of features. Timelace strips that away. It is a small, from-scratch, content-addressed version control system where the entire object model fits in one file you can actually read in five minutes.

## What it is

Timelace is a command-line VCS with the core git-shaped workflow: `init`, `add`, `commit`, `log`, `status`, `checkout`. No branches, no remotes, no merge, no packfiles. Just the object store and the commit graph, done properly.

## The object model

Everything Timelace stores is an **object**, and every object's identity is the SHA-256 hash of its own encoded bytes. That's what "content-addressed" means: the name of a thing *is* a hash of what it contains, so two pieces of identical content always resolve to the same object, automatically, with no explicit deduplication logic required.

There are three kinds of objects:

- **Blob**, the raw bytes of one file's contents. Nothing else.
- **Tree**, a directory snapshot: a sorted list of entries, each one a name paired with the object id of a blob (a file) or another tree (a subdirectory).
- **Commit**, a tree id (the root snapshot), an optional parent commit id, a message, and a timestamp.

Each object is encoded with a small header before hashing:

```
<kind> <byte-length>\0<body>
```

The header participates in the hash, so a blob and a tree that happen to contain the same bytes never collide. The hash of that whole encoded buffer, hex-encoded, is the object's id, and that id is also its filename on disk.

## On-disk layout

```
.timelace/
  objects/<first 2 hex chars>/<remaining 62 hex chars>   one file per object
  HEAD                                                    id of the latest commit
  index                                                   paths staged for the next commit
```

`HEAD` is a single object id pointing at the tip of the commit history. Walking from `HEAD` through each commit's `parent` field is the entire log.

## Walkthrough

```
$ timelace init
initialized empty timelace repository in ./.timelace

$ echo "hello" > notes.txt
$ timelace add notes.txt
$ timelace commit -m "first commit"
committed 4b7b6338afbc1a9b9b0fbaae5866a8483f70ec56f0d0b2630e4955788624edc9

$ echo "hello again" > notes.txt
$ timelace add notes.txt
$ timelace commit -m "second commit"
committed 1ca8312577a85d387f38460fab46a8dd357367a43e8e707d9dc7100e627c8a50

$ timelace log
commit 1ca8312577a85d387f38460fab46a8dd357367a43e8e707d9dc7100e627c8a50
timestamp 1787349638

    second commit

commit 4b7b6338afbc1a9b9b0fbaae5866a8483f70ec56f0d0b2630e4955788624edc9
timestamp 1787349600

    first commit

$ timelace checkout 4b7b6338afbc1a9b9b0fbaae5866a8483f70ec56f0d0b2630e4955788624edc9
$ cat notes.txt
hello
```

Two files with identical content always share one blob object, verified in the test suite: stage two different files with the same bytes, commit, and the tree records one object id for both.

## Usage

```
timelace init                    create a repository in the current directory
timelace add <path>               stage a file, or every file in a directory
timelace commit -m "<message>"    snapshot the staged files as a new commit
timelace log                      walk the commit history from HEAD
timelace status                   show staged vs. untracked files
timelace checkout <commit-id>     restore the working tree to a given commit
```

## Building

```
cargo build --release
```

The only dependencies are `sha2` for hashing and `clap` for argument parsing. Everything else, the object model, the store, the commit graph, is written from scratch.

## Testing

```
cargo test
```

The integration suite covers: repository initialization, staging and committing, parent-chain linking across commits, log ordering, content deduplication, and exact working-tree restoration on checkout, plus error handling for operating outside a repository, unknown commit ids, and committing with nothing staged.

By Pavan Nallamothu.
