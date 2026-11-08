//! Line-based TCP front end.
//!
//! ```text
//! PUT <key> <value...>   -> OK
//! GET <key>              -> VALUE <value> | NOT_FOUND
//! DEL <key>              -> OK | NOT_FOUND
//! SCAN <prefix>          -> one "<key> <value>" line per match, then END
//! COMPACT                -> OK
//! QUIT                   -> closes the connection
//! ```
//! Keys cannot contain spaces or newlines. Values may contain spaces but not
//! newlines. Errors reply `ERR <message>`.

use crate::storage::Storage;
use crate::store::Store;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

pub fn serve<S: Storage + Send + 'static>(
    listener: TcpListener,
    store: Arc<Mutex<Store<S>>>,
) -> std::io::Result<()> {
    for conn in listener.incoming() {
        let conn = conn?;
        let store = Arc::clone(&store);
        thread::spawn(move || {
            let _ = handle(conn, &store);
        });
    }
    Ok(())
}

fn handle<S: Storage>(conn: TcpStream, store: &Mutex<Store<S>>) -> std::io::Result<()> {
    let mut out = conn.try_clone()?;
    for line in BufReader::new(conn).lines() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let mut s = store.lock().unwrap_or_else(|e| e.into_inner());
        match cmd.to_ascii_uppercase().as_str() {
            "PUT" => match rest.split_once(' ') {
                Some((k, v)) if !k.is_empty() => match s.put(k.as_bytes(), v.as_bytes()) {
                    Ok(()) => writeln!(out, "OK")?,
                    Err(e) => writeln!(out, "ERR {e}")?,
                },
                _ => writeln!(out, "ERR usage: PUT <key> <value>")?,
            },
            "GET" => match s.get(rest.as_bytes()) {
                Some(v) => writeln!(out, "VALUE {}", String::from_utf8_lossy(v))?,
                None => writeln!(out, "NOT_FOUND")?,
            },
            "DEL" => match s.delete(rest.as_bytes()) {
                Ok(true) => writeln!(out, "OK")?,
                Ok(false) => writeln!(out, "NOT_FOUND")?,
                Err(e) => writeln!(out, "ERR {e}")?,
            },
            "SCAN" => {
                for (k, v) in s.scan_prefix(rest.as_bytes()) {
                    writeln!(
                        out,
                        "{} {}",
                        String::from_utf8_lossy(k),
                        String::from_utf8_lossy(v)
                    )?;
                }
                writeln!(out, "END")?;
            }
            "COMPACT" => match s.compact() {
                Ok(()) => writeln!(out, "OK")?,
                Err(e) => writeln!(out, "ERR {e}")?,
            },
            "QUIT" => return Ok(()),
            "" => {}
            _ => writeln!(out, "ERR unknown command")?,
        }
    }
    Ok(())
}
