use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};
use tempfile::NamedTempFile;
type Result<T> = std::result::Result<T, String>;
fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn atomic(path: &Path, bytes: &[u8], create: bool) -> Result<()> {
    write_atomic(path, bytes, create, None)
}

/// Bound wiki pages can be opened by the installer even when Docker writes as another UID.
pub fn wiki_page(path: &Path, bytes: &[u8], root: &Path) -> Result<()> {
    write_atomic(path, bytes, false, wiki_user(root)?)
}

fn wiki_user(root: &Path) -> Result<Option<u32>> {
    if root != Path::new("/data/wiki") {
        return Ok(None);
    }
    let user = std::env::var("LIBRARY_WIKI_USER")
        .ok()
        .map(|v| v.parse::<u32>().map_err(io))
        .transpose()?;
    Ok(user.filter(|id| *id != rustix::process::geteuid().as_raw()))
}

pub fn create_wiki_page(path: &Path, bytes: &[u8], root: &Path) -> Result<()> {
    write_atomic(path, bytes, true, wiki_user(root)?)
}

pub fn probe_wiki(root: &Path) -> Result<()> {
    let probe = NamedTempFile::new_in(root).map_err(|e| format!("wiki is not writable: {e}"))?;
    if let Some(uid) = wiki_user(root)? {
        grant_wiki_access(probe.path(), uid)?;
    }
    probe.close().map_err(io)
}

fn grant_wiki_access(path: &Path, uid: u32) -> Result<()> {
    let status = std::process::Command::new("setfacl")
        .args([
            "--set",
            &format!("u::rw-,u:{uid}:rw-,g::---,m::rw-,o::---"),
            "--",
        ])
        .arg(path)
        .status()
        .map_err(io)?;
    if !status.success() {
        return Err(
            "cannot grant the host user access to the wiki page; check filesystem ACL support"
                .into(),
        );
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8], create: bool, wiki_user: Option<u32>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent).map_err(io)?;
    temp.write_all(bytes).map_err(io)?;
    if let Ok(meta) = fs::metadata(path) {
        temp.as_file()
            .set_permissions(meta.permissions())
            .map_err(io)?;
    }
    if let Some(uid) = wiki_user {
        // Preserve ACLs as well as mode bits when replacing a previously shared page.
        if path.exists() {
            let status = std::process::Command::new("cp")
                .args(["--attributes-only", "--preserve=mode", "--"])
                .arg(path)
                .arg(temp.path())
                .status()
                .map_err(io)?;
            if !status.success() {
                return Err("could not preserve wiki page permissions".into());
            }
        } else {
            grant_wiki_access(temp.path(), uid)?;
        }
    }
    temp.as_file().sync_all().map_err(io)?;
    if create {
        temp.persist_noclobber(path)
    } else {
        temp.persist(path)
    }
    .map_err(io)?;
    File::open(parent).and_then(|f| f.sync_all()).map_err(io)
}

pub fn lock(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(io)?;
    f.lock().map_err(io)?;
    Ok(f)
}

pub fn no_link(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(m) if m.is_symlink() => Err(format!("refusing symlink: {}", path.display())),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn shared_filing_access_survives_replacement_and_settings_stay_private() {
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("filing.md");
        let uid = if rustix::process::geteuid().as_raw() == 12345 {
            12346
        } else {
            12345
        };
        let acl = |p: &Path| {
            let out = std::process::Command::new("getfacl")
                .args(["-cpn", "--"])
                .arg(p)
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap()
        };
        write_atomic(&page, b"first", false, Some(uid)).unwrap();
        let first = acl(&page);
        for entry in [
            format!("user:{uid}:rw-"),
            "group::---".into(),
            "other::---".into(),
        ] {
            assert!(first.lines().any(|line| line == entry));
        }
        write_atomic(&page, b"second", false, Some(uid)).unwrap();
        assert_eq!(fs::read(&page).unwrap(), b"second");
        assert_eq!(acl(&page), first);
        let settings = dir.path().join("settings");
        atomic(&settings, b"private", true).unwrap();
        assert_eq!(
            settings.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
