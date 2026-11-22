//! Local stand-ins for the user-configured report sink and catalog source.
use std::{
    env, fs,
    io::{self, Read},
};
fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("receive") => {
            let mut text = String::new();
            io::stdin().read_to_string(&mut text).unwrap();
            let title = env::var("LIBRARY_REPORT_TITLE").unwrap();
            fs::write(args.next().unwrap(), format!("{title}|{text}")).unwrap();
        }
        Some("labels") => println!(r#"[{{"label":"one"}}]"#),
        Some("unavailable") => std::process::exit(3),
        _ => std::process::exit(2),
    }
}
