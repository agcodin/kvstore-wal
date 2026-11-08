use kvstore_wal::{server, Options, Store, SyncPolicy};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

fn start(path: &std::path::Path) -> std::net::SocketAddr {
    let o = Options {
        sync: SyncPolicy::Always,
        auto_compact_min_bytes: None,
    };
    let store = Store::open_with(kvstore_wal::FileStorage::open(path).unwrap(), o).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || server::serve(l, Arc::new(Mutex::new(store))));
    addr
}

fn ask(c: &mut TcpStream, r: &mut BufReader<TcpStream>, cmd: &str) -> String {
    writeln!(c, "{cmd}").unwrap();
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    s.trim_end().to_string()
}

#[test]
fn protocol_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wal.log");
    let addr = start(&path);
    let mut c = TcpStream::connect(addr).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    assert_eq!(ask(&mut c, &mut r, "PUT user:1 alice smith"), "OK");
    assert_eq!(ask(&mut c, &mut r, "PUT user:2 bob"), "OK");
    assert_eq!(ask(&mut c, &mut r, "GET user:1"), "VALUE alice smith");
    assert_eq!(ask(&mut c, &mut r, "GET nope"), "NOT_FOUND");
    assert_eq!(ask(&mut c, &mut r, "DEL user:2"), "OK");
    assert_eq!(ask(&mut c, &mut r, "DEL user:2"), "NOT_FOUND");
    assert_eq!(ask(&mut c, &mut r, "PUT other x"), "OK");
    writeln!(c, "SCAN user:").unwrap();
    let mut l = String::new();
    r.read_line(&mut l).unwrap();
    assert_eq!(l.trim_end(), "user:1 alice smith");
    l.clear();
    r.read_line(&mut l).unwrap();
    assert_eq!(l.trim_end(), "END");
    assert_eq!(ask(&mut c, &mut r, "COMPACT"), "OK");
    assert_eq!(ask(&mut c, &mut r, "BOGUS"), "ERR unknown command");

    // A second server on the same file sees the data.
    let addr2 = start(&path);
    let mut c2 = TcpStream::connect(addr2).unwrap();
    let mut r2 = BufReader::new(c2.try_clone().unwrap());
    assert_eq!(ask(&mut c2, &mut r2, "GET user:1"), "VALUE alice smith");
}
