//! Locations used by the public compatibility tests.
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_owned()
}

pub fn python() -> OsString {
    env::var_os("BENEDICT_TEST_PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    })
}
