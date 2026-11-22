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

pub fn read(root: &Path, name: &str) -> Result<(String, String)> {
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
            let mut text = String::new();
            original(&dir, part)?
                .read_to_string(&mut text)
                .map_err(|_| "read requires a UTF-8 shelf original")?;
            if text.contains('\0') {
                return Err("read requires a text shelf original".into());
            }
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            return Ok((text, hash));
        }
    }
    Err("empty shelf path".into())
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
