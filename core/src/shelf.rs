//! Open each original relative to an already-open shelf directory. Reject symlinks
//! in every component so an untrusted shelf cannot redirect a read during indexing.
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat};
use sha2::{Digest, Sha256};
use std::{
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
    Binary(Vec<u8>, String),
}

/// Most pages one read may convert, so a reader takes the part it needs.
pub const MAX_PAGES: usize = 5;

/// Pages named by a range such as `3-5` or `1,3`; `None` when the range is malformed.
pub fn page_count(pages: &str) -> Option<usize> {
    let mut total = 0;
    for part in pages.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
        if a == 0 || b < a {
            return None;
        }
        total += b - a + 1;
    }
    Some(total)
}

/// Convert the requested pages of a non-text original (PDF, HWP, HWPX, DOCX, XLSX,
/// image) to Markdown with kordoc when `BENEDICT_KORDOC` names its command-line
/// script. Pages without a text layer are read with kordoc's built-in OCR.
pub fn convert(bytes: &[u8], name: &str, pages: &str) -> Result<Option<String>> {
    match page_count(pages) {
        Some(n) if n <= MAX_PAGES => {}
        Some(_) => return Err(format!("read at most {MAX_PAGES} pages at a time")),
        None => return Err("pages must look like 3-5 or 1,3".into()),
    }
    let Some(cli) = std::env::var_os("BENEDICT_KORDOC") else {
        return Ok(None);
    };
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin");
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = dir.path().join(format!("original.{ext}"));
    let output = dir.path().join("original.md");
    std::fs::write(&input, bytes).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("node")
        .arg(&cli)
        .arg(&input)
        .args(["--ocr", "-p", pages, "-o"])
        .arg(&output)
        .env("HOME", std::env::temp_dir().join("benedict-kordoc"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("kordoc: {e}"))?;
    if !status.success() {
        return Err("kordoc could not read this original".into());
    }
    std::fs::read_to_string(&output)
        .map(Some)
        .map_err(|_| "kordoc produced no text".into())
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
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(Original::Text(text, hash)),
        Ok(text) => Ok(Original::Binary(text.into_bytes(), hash)),
        Err(e) => Ok(Original::Binary(e.into_bytes(), hash)),
    }
}

/// Hand each text original to `visit` as ("shelf:<path>", text), one at a time.
pub fn each(root: &Path, visit: &mut dyn FnMut(String, String) -> Result<()>) -> Result<()> {
    walk(&directory(root)?, "", visit)
}

fn walk(dir: &File, prefix: &str, visit: &mut dyn FnMut(String, String) -> Result<()>) -> Result<()> {
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
                walk(&sub, &format!("{rel}/"), visit)?;
            }
            FileType::RegularFile
                if matches!(
                    Path::new(name).extension().and_then(|e| e.to_str()),
                    Some("md" | "txt")
                ) =>
            {
                let mut text = String::new();
                match original(dir, name.as_ref())?.read_to_string(&mut text) {
                    Ok(_) if !text.contains('\0') => visit(format!("shelf:{rel}"), text)?,
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
                assert_eq!(bytes, pdf.to_vec());
                assert_eq!(sha256, format!("{:x}", Sha256::digest(pdf)));
            }
            Original::Text(..) => panic!("binary original read as text"),
        }
        assert!(read(tmp.path(), "reports/scan.pdf").is_err());
        assert!(open_original(tmp.path(), "../outside.pdf").is_err());
    }

    #[test]
    fn a_page_range_is_small_and_well_formed() {
        assert_eq!(page_count("3-5"), Some(3));
        assert_eq!(page_count("1,3,7-8"), Some(4));
        assert_eq!(page_count("0-2"), None);
        assert_eq!(page_count("5-3"), None);
        assert_eq!(page_count("1;rm"), None);
        assert!(convert(b"%PDF", "a.pdf", "1-6").is_err());
        assert!(convert(b"%PDF", "a.pdf", "x").is_err());
    }
}
