use sha2::{Digest, Sha256};
use std::fmt;

/// A content-address: the hex-encoded SHA-256 digest of an object's encoded bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub String);

impl ObjectId {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        ObjectId(hex_encode(&digest))
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("'{s}' is not a valid object id (want 64 hex chars)"));
        }
        Ok(ObjectId(s.to_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A tree entry: one named child, either a blob (file) or a nested tree (directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: String,
    pub is_tree: bool,
    pub id: ObjectId,
}

/// The three object kinds stored in the object store. Each variant knows how to
/// encode itself to bytes; the ObjectId of an object is the SHA-256 of that encoding,
/// so identical content always produces the same id (content addressing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Vec<TreeEntry>),
    Commit {
        tree: ObjectId,
        parent: Option<ObjectId>,
        message: String,
        timestamp: u64,
    },
}

impl Object {
    /// Serialize to the exact bytes that get hashed and stored.
    /// Format: "<kind> <len>\0<body>" mirroring git's loose-object framing,
    /// so the header itself is content that participates in the hash.
    pub fn encode(&self) -> Vec<u8> {
        let body = self.encode_body();
        let kind = self.kind();
        let mut out = Vec::with_capacity(body.len() + 16);
        out.extend_from_slice(kind.as_bytes());
        out.push(b' ');
        out.extend_from_slice(body.len().to_string().as_bytes());
        out.push(0);
        out.extend_from_slice(&body);
        out
    }

    pub fn id(&self) -> ObjectId {
        ObjectId::from_bytes(&self.encode())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Object::Blob(_) => "blob",
            Object::Tree(_) => "tree",
            Object::Commit { .. } => "commit",
        }
    }

    fn encode_body(&self) -> Vec<u8> {
        match self {
            Object::Blob(data) => data.clone(),
            Object::Tree(entries) => {
                let mut sorted = entries.clone();
                sorted.sort_by(|a, b| a.name.cmp(&b.name));
                let mut out = Vec::new();
                for e in &sorted {
                    let kind = if e.is_tree { "tree" } else { "blob" };
                    out.extend_from_slice(
                        format!("{kind} {} {}\n", e.id.as_str(), e.name).as_bytes(),
                    );
                }
                out
            }
            Object::Commit {
                tree,
                parent,
                message,
                timestamp,
            } => {
                let mut out = String::new();
                out.push_str(&format!("tree {}\n", tree.as_str()));
                if let Some(p) = parent {
                    out.push_str(&format!("parent {}\n", p.as_str()));
                }
                out.push_str(&format!("timestamp {timestamp}\n"));
                out.push('\n');
                out.push_str(message);
                out.into_bytes()
            }
        }
    }

    /// Parse bytes previously produced by `encode` back into an Object.
    pub fn decode(bytes: &[u8]) -> Result<Object, String> {
        let nul = bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or("malformed object: no header terminator")?;
        let header = std::str::from_utf8(&bytes[..nul]).map_err(|_| "malformed header")?;
        let mut parts = header.splitn(2, ' ');
        let kind = parts.next().ok_or("malformed header: missing kind")?;
        let len: usize = parts
            .next()
            .ok_or("malformed header: missing length")?
            .parse()
            .map_err(|_| "malformed header: bad length")?;
        let body = &bytes[nul + 1..];
        if body.len() != len {
            return Err(format!(
                "malformed object: header says {len} bytes, found {}",
                body.len()
            ));
        }
        match kind {
            "blob" => Ok(Object::Blob(body.to_vec())),
            "tree" => {
                let text = std::str::from_utf8(body).map_err(|_| "malformed tree body")?;
                let mut entries = Vec::new();
                for line in text.lines() {
                    let mut fields = line.splitn(3, ' ');
                    let kind = fields.next().ok_or("malformed tree entry")?;
                    let id = fields.next().ok_or("malformed tree entry")?;
                    let name = fields.next().ok_or("malformed tree entry")?;
                    entries.push(TreeEntry {
                        name: name.to_string(),
                        is_tree: kind == "tree",
                        id: ObjectId::parse(id)?,
                    });
                }
                Ok(Object::Tree(entries))
            }
            "commit" => {
                let text = std::str::from_utf8(body).map_err(|_| "malformed commit body")?;
                let (header_part, message) = text
                    .split_once("\n\n")
                    .ok_or("malformed commit: missing message separator")?;
                let mut tree = None;
                let mut parent = None;
                let mut timestamp = None;
                for line in header_part.lines() {
                    if let Some(rest) = line.strip_prefix("tree ") {
                        tree = Some(ObjectId::parse(rest)?);
                    } else if let Some(rest) = line.strip_prefix("parent ") {
                        parent = Some(ObjectId::parse(rest)?);
                    } else if let Some(rest) = line.strip_prefix("timestamp ") {
                        timestamp = Some(
                            rest.parse::<u64>()
                                .map_err(|_| "malformed commit: bad timestamp")?,
                        );
                    }
                }
                Ok(Object::Commit {
                    tree: tree.ok_or("malformed commit: missing tree")?,
                    parent,
                    message: message.to_string(),
                    timestamp: timestamp.ok_or("malformed commit: missing timestamp")?,
                })
            }
            other => Err(format!("unknown object kind '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_blobs_share_id() {
        let a = Object::Blob(b"hello world".to_vec());
        let b = Object::Blob(b"hello world".to_vec());
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn different_blobs_differ() {
        let a = Object::Blob(b"hello".to_vec());
        let b = Object::Blob(b"world".to_vec());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn roundtrip_blob() {
        let obj = Object::Blob(b"some content".to_vec());
        let encoded = obj.encode();
        let decoded = Object::decode(&encoded).unwrap();
        assert_eq!(obj, decoded);
    }

    #[test]
    fn roundtrip_tree() {
        let obj = Object::Tree(vec![TreeEntry {
            name: "a.txt".into(),
            is_tree: false,
            id: ObjectId::from_bytes(b"blob 5\0hello"),
        }]);
        let encoded = obj.encode();
        let decoded = Object::decode(&encoded).unwrap();
        assert_eq!(obj, decoded);
    }

    #[test]
    fn roundtrip_commit() {
        let obj = Object::Commit {
            tree: ObjectId::from_bytes(b"tree"),
            parent: None,
            message: "first commit".into(),
            timestamp: 42,
        };
        let encoded = obj.encode();
        let decoded = Object::decode(&encoded).unwrap();
        assert_eq!(obj, decoded);
    }

    #[test]
    fn bad_object_id_rejected() {
        assert!(ObjectId::parse("not-a-hash").is_err());
    }
}
