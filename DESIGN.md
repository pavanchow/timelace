# Timelace design

## Goals

A version control system small enough to read end to end, built around the same idea that makes git work: content addressing. Nothing here is a simplified imitation of git's plumbing, it is the same idea implemented directly, with the extra layers (packfiles, refs, index caching, ahead/behind tracking) left out because they are not the point.

## Object model

Three object kinds, defined in `src/objects.rs`.

**Blob.** The raw bytes of a single file. No filename, no metadata, just content. Two files with the same bytes produce the same blob, whatever they are named or wherever they live.

**Tree.** A directory snapshot. A tree is a list of entries, each one a name, a flag for whether the entry is a file or a nested tree, and the object id of that child. Entries are sorted by name before encoding, so the same directory contents always produce the same tree object regardless of the order files were added in.

**Commit.** A tree id (the root of that commit's snapshot), an optional parent commit id, a message, and a Unix timestamp. A commit with no parent is the first commit in the repository. Following `parent` links from any commit walks backward through history.

## Content addressing

Every object is encoded to bytes before it is hashed or stored. The encoding is a small header followed by the body:

```
<kind> <byte-length-of-body>\0<body>
```

`kind` is `blob`, `tree`, or `commit`. The header is part of what gets hashed, so a blob containing the literal bytes of some other object's body still hashes differently (the header disambiguates it). The hash function is SHA-256 (`sha2` crate), and the resulting digest, hex-encoded, is the object's id.

Because the id is a deterministic function of the encoded bytes, storing an object twice with identical content always resolves to the same id. That's the whole mechanism behind deduplication: there is no explicit "check if this already exists" step conceptually, the id itself already tells you where the object would live, and if it is already there, writing it again is a no-op.

Tree encoding sorts entries by name first. This matters: without sorting, adding the same two files to a tree in a different order would produce a different byte sequence and therefore a different tree id, even though the directory contents are identical. Sorting makes the tree id depend only on content, not on insertion order.

Commit encoding is a small header block (`tree <id>`, optional `parent <id>`, `timestamp <n>`) followed by a blank line and the raw message, mirroring the blob/tree header-then-body shape one level up.

## On-disk layout

```
.timelace/
  objects/<id[0..2]>/<id[2..]>    one file per object, content is the encoded bytes
  HEAD                            the id of the latest commit, or empty if none exists yet
  index                           newline-separated list of paths staged for the next commit
```

Splitting the object id into a 2-character directory prefix and the remaining 62 characters as the filename is the same trick git uses: it keeps any single directory from accumulating tens of thousands of entries as the repository grows.

## HEAD and the index

`HEAD` holds exactly one commit id (or is empty in a freshly initialized repository). There is no notion of branches, `HEAD` always just means "the latest commit."

The `index` file is the staging list: `add` appends a path to it, `commit` reads the whole list and uses it to build the tree for the new snapshot. The index is deliberately simple, it is a persistent list of tracked paths rather than a one-shot staging area that gets cleared after each commit. That means once a path has been added, every future commit re-snapshots its current on-disk contents automatically, without needing to `add` it again unless a brand-new path is introduced.

## Commit algorithm

1. Read the index for the list of staged paths.
2. Build a tree: paths are split on `/` and folded into a nested structure of directories, each directory turned into a `Tree` object built bottom-up (children first, so a directory's tree id can be computed from already-known child ids), each file read from disk and turned into a `Blob`.
3. Every blob and tree gets written to the object store as it's built. Identical content along the way (two files with the same bytes, or two commits that snapshot an unchanged subdirectory) simply reuses the object already on disk.
4. Read the current `HEAD` as this commit's parent (or none, for the first commit).
5. Build and store the `Commit` object, then write its id as the new `HEAD`.

## Checkout algorithm

1. Read the target commit object and pull out its tree id. An id that doesn't resolve to a commit is reported as an error, not a panic.
2. Delete every tracked file currently in the working directory (everything outside `.timelace`), then remove any directories left empty by that deletion.
3. Walk the target tree recursively: for each entry, create the directory (if it's a nested tree) or write the blob's bytes to disk (if it's a file), rebuilding the exact working-tree state the commit recorded.
4. Point `HEAD` at the checked-out commit.

Deleting before restoring is what makes checkout exact: a file that existed in the current working tree but not in the target commit is correctly gone afterward, not left behind as stray unstaged content.

## Error handling

Every operation that can fail returns a `Result`, there is no `unwrap`-to-panic path in the command layer. `Store::discover` walks up from the current directory looking for `.timelace` and returns a clear "not a repository" error if it hits the filesystem root without finding one. Reading an object that doesn't exist, decoding a malformed object, parsing a string that isn't a valid 64-character hex id, and committing with nothing staged all return descriptive errors through the same `StoreError` type instead of crashing.
