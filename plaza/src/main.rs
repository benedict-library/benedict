use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use plaza::accounts::Accounts;
use plaza::client::{self, USAGE};
use plaza::store::Store;
use plaza::web::{self, Square};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// PLAZA_ACCOUNTS: the accounts file. PLAZA_DATA: the folder holding events.jsonl. PLAZA_LISTEN: address:port.
async fn serve() -> Result<(), String> {
    let accounts = Accounts::load(&env("PLAZA_ACCOUNTS").ok_or("set PLAZA_ACCOUNTS (the accounts file)")?)?;
    let data = PathBuf::from(env("PLAZA_DATA").ok_or("set PLAZA_DATA (the folder for events.jsonl)")?);
    std::fs::create_dir_all(&data).map_err(|e| format!("cannot make {}: {e}", data.display()))?;
    let store = Store::open(&data.join("events.jsonl"), &accounts)?;
    let listen = env("PLAZA_LISTEN").unwrap_or_else(|| "127.0.0.1:43190".into());
    let listener = tokio::net::TcpListener::bind(&listen).await.map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    eprintln!("plaza: {} posts, {} events, listening on {listen}", store.posts.len(), store.events.len());
    web::serve(listener, Arc::new(Square { store: Mutex::new(store), accounts })).await.map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let result = match argv.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("serve") => serve().await,
        Some(_) => client::run(&argv).await,
    };
    if let Err(e) = result {
        eprintln!("plaza: {e}");
        std::process::exit(1);
    }
}
