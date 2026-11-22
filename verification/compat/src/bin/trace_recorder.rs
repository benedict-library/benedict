//! Synthetic trace endpoint used only by the compatibility suite.
use std::{collections::BTreeMap, env, fs, path::PathBuf};
fn main() {
    let mut args = env::args().skip(1);
    let directory = PathBuf::from(args.next().expect("trace directory"));
    let mut fields = BTreeMap::new();
    while let Some(key) = args.next() {
        fields.insert(key, args.next().expect("trace argument value"));
    }
    let tmp = tempfile::NamedTempFile::new_in(&directory).unwrap();
    serde_json::to_writer(tmp.as_file(), &fields).unwrap();
    let final_name = tmp.path().with_extension("json");
    fs::rename(tmp.path(), final_name).unwrap();
}
