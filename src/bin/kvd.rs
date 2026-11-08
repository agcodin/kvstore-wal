use kvstore_wal::{server, Store};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() {
    let mut dir = PathBuf::from("data");
    let mut addr = "127.0.0.1:7878".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("--dir", Some(v)) => dir = v.into(),
            ("--addr", Some(v)) => addr = v,
            _ => {
                eprintln!("usage: kvd [--dir DIR] [--addr HOST:PORT]");
                std::process::exit(2);
            }
        }
    }
    std::fs::create_dir_all(&dir).expect("create data dir");
    let store = Store::open(dir.join("wal.log")).expect("open store");
    let r = store.recovery();
    eprintln!(
        "recovered {} records ({} bytes), discarded {} bytes",
        r.records, r.valid_bytes, r.discarded_bytes
    );
    let listener = TcpListener::bind(&addr).expect("bind");
    eprintln!("listening on {addr}");
    server::serve(listener, Arc::new(Mutex::new(store))).expect("serve");
}
