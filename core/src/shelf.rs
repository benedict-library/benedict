//! Open each original relative to an already-open shelf directory. Reject symlinks
//! in every component so an untrusted shelf cannot redirect a read during indexing.
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path},
};

type Result<T> = std::result::Result<T, String>;
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn directory(path: &Path) -> Result<File> {
    open(path, DIRECTORY, Mode::empty())
        .map(File::from)
        .map_err(|_| "cannot open configured shelf".into())
}

fn original(dir: &File, name: &std::ffi::OsStr) -> Result<File> {
    let fd = openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "shelf original unavailable")?;
    let file = File::from(fd);
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular shelf original".into());
    }
    Ok(file)
}

/// A shelf original as read: text with its SHA-256, or a non-text original
/// (PDF, office document, image) described by its size and SHA-256 so the
/// agent can open the file itself.
pub enum Original {
    Text(String, String),
    Binary(u64, String),
}

fn locate(root: &Path, name: &str) -> Result<File> {
    let mut dir = directory(root)?;
    let mut components = Path::new(name).components().peekable();
    while let Some(c) = components.next() {
        let Component::Normal(part) = c else {
            return Err("invalid shelf path".into());
        };
        if part.as_encoded_bytes().starts_with(b".") {
            return Err("invalid shelf path".into());
        }
        if components.peek().is_some() {
            dir = File::from(
                openat(&dir, part, DIRECTORY, Mode::empty())
                    .map_err(|_| "shelf original unavailable")?,
            );
        } else {
            return original(&dir, part);
        }
    }
    Err("empty shelf path".into())
}

pub fn read(root: &Path, name: &str) -> Result<(String, String)> {
    match open_original(root, name)? {
        Original::Text(text, hash) => Ok((text, hash)),
        Original::Binary(..) => Err("read requires a text shelf original".into()),
    }
}

pub fn open_original(root: &Path, name: &str) -> Result<Original> {
    let mut bytes = Vec::new();
    locate(root, name)?
        .read_to_end(&mut bytes)
        .map_err(|_| "shelf original unavailable")?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let size = bytes.len() as u64;
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(Original::Text(text, hash)),
        _ => Ok(Original::Binary(size, hash)),
    }
}

pub fn index(root: &Path, docs: &mut BTreeMap<String, String>) -> Result<()> {
    walk(&directory(root)?, "", docs)
}

fn walk(dir: &File, prefix: &str, docs: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in Dir::read_from(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let rel = format!("{prefix}{name}");
        // Network file stores may leave the directory entry type unknown.
        let kind = match entry.file_type() {
            FileType::Unknown => FileType::from_raw_mode(
                statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|e| e.to_string())?
                    .st_mode,
            ),
            known => known,
        };
        match kind {
            FileType::Directory => {
                let sub = File::from(
                    openat(dir, name, DIRECTORY, Mode::empty()).map_err(|e| e.to_string())?,
                );
                walk(&sub, &format!("{rel}/"), docs)?;
            }
            FileType::RegularFile
                if matches!(
                    Path::new(name).extension().and_then(|e| e.to_str()),
                    Some("md" | "txt")
                ) =>
            {
                let mut text = String::new();
                match original(dir, name.as_ref())?.read_to_string(&mut text) {
                    Ok(_) if !text.contains('\0') => {
                        docs.insert(format!("shelf:{rel}"), text);
                    }
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => (),
                    Err(e) => return Err(e.to_string()),
                }
            }
            _ => (),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_original_is_described_instead_of_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("reports")).unwrap();
        std::fs::write(tmp.path().join("reports/notes.txt"), "plain words").unwrap();
        let pdf = b"%PDF-1.7\n\xff\xfe\x00binary";
        std::fs::write(tmp.path().join("reports/scan.pdf"), pdf).unwrap();

        match open_original(tmp.path(), "reports/notes.txt").unwrap() {
            Original::Text(text, _) => assert_eq!(text, "plain words"),
            Original::Binary(..) => panic!("text original read as binary"),
        }
        match open_original(tmp.path(), "reports/scan.pdf").unwrap() {
            Original::Binary(bytes, sha256) => {
                assert_eq!(bytes, pdf.len() as u64);
                assert_eq!(sha256, format!("{:x}", Sha256::digest(pdf)));
            }
            Original::Text(..) => panic!("binary original read as text"),
        }
        assert!(read(tmp.path(), "reports/scan.pdf").is_err());
        assert!(open_original(tmp.path(), "../outside.pdf").is_err());
    }
}
