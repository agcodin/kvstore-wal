//! Crash recovery sweeps over a real file: truncate or corrupt the log at every
//! byte offset and check the store recovers to a valid prefix of the history.

use kvstore_wal::{FileStorage, Options, Store, SyncPolicy};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

type Model = BTreeMap<Vec<u8>, Vec<u8>>;

fn opts() -> Options {
    Options {
        sync: SyncPolicy::Always,
        auto_compact_min_bytes: None,
    }
}

fn dump(s: &Store) -> Model {
    s.scan_prefix(b"")
        .map(|(k, v)| (k.to_vec(), v.to_vec()))
        .collect()
}

fn open(path: &Path) -> Store {
    Store::open_with(FileStorage::open(path).unwrap(), opts()).unwrap()
}

/// Build a log with a mix of puts, overwrites and deletes. Returns the log
/// bytes, the byte offset after each record, and the model after each record.
fn build(dir: &Path) -> (Vec<u8>, Vec<usize>, Vec<Model>) {
    let path = dir.join("wal.log");
    let mut s = open(&path);
    let mut model = Model::new();
    let mut ends = vec![0usize];
    let mut models = vec![model.clone()];
    for i in 0..30u32 {
        // Deletes target the key written by the previous step, so they always
        // hit a live key and always write a record.
        let k = if i % 5 == 4 {
            format!("key{}", (i - 1) % 7)
        } else {
            format!("key{}", i % 7)
        }
        .into_bytes();
        if i % 5 == 4 {
            assert!(s.delete(&k).unwrap());
            model.remove(&k);
        } else {
            let v = vec![i as u8; (i as usize * 3) % 17];
            s.put(&k, &v).unwrap();
            model.insert(k, v);
        }
        ends.push(s.log_len() as usize);
        models.push(model.clone());
    }
    drop(s);
    (fs::read(&path).unwrap(), ends, models)
}

#[test]
fn truncate_at_every_offset() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, ends, models) = build(dir.path());
    eprintln!("log is {} bytes, {} records", bytes.len(), ends.len() - 1);
    let mut cases = 0;
    for cut in 0..=bytes.len() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("wal.log");
        fs::write(&path, &bytes[..cut]).unwrap();

        // Largest record boundary at or before the cut.
        let n = ends.iter().rposition(|&e| e <= cut).unwrap();
        let mut s = open(&path);
        assert_eq!(dump(&s), models[n], "cut={cut}");
        assert_eq!(s.recovery().valid_bytes as usize, ends[n], "cut={cut}");
        assert_eq!(
            s.recovery().discarded_bytes as usize,
            cut - ends[n],
            "cut={cut}"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len() as usize,
            ends[n],
            "cut={cut}"
        );

        // A write after recovery must survive the next restart.
        s.put(b"post-crash", b"ok").unwrap();
        drop(s);
        let s = open(&path);
        assert_eq!(s.get(b"post-crash"), Some(&b"ok"[..]), "cut={cut}");
        let mut want = models[n].clone();
        want.insert(b"post-crash".to_vec(), b"ok".to_vec());
        assert_eq!(dump(&s), want, "cut={cut}");
        cases += 1;
    }
    assert_eq!(cases, bytes.len() + 1);
}

#[test]
fn corrupt_every_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, ends, models) = build(dir.path());
    for i in 0..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let d = tempfile::tempdir().unwrap();
            let path = d.path().join("wal.log");
            let mut b = bytes.clone();
            b[i] ^= mask;
            fs::write(&path, &b).unwrap();

            // The damaged byte sits in record j; everything before it survives.
            let j = ends.iter().rposition(|&e| e <= i).unwrap();
            let mut s = open(&path);
            assert_eq!(dump(&s), models[j], "byte={i} mask={mask:#x}");
            assert_eq!(
                s.recovery().valid_bytes as usize,
                ends[j],
                "byte={i} mask={mask:#x}"
            );

            s.put(b"post-crash", b"ok").unwrap();
            drop(s);
            assert_eq!(open(&path).get(b"post-crash"), Some(&b"ok"[..]), "byte={i}");
        }
    }
}

#[test]
fn crash_during_compaction_leaves_old_or_new() {
    // A stale temp file from an interrupted compaction must be ignored.
    let dir = tempfile::tempdir().unwrap();
    let (bytes, _ends, models) = build(dir.path());
    let path = dir.path().join("wal.log");
    fs::write(
        dir.path().join("wal.log.compact.tmp"),
        b"half written garbage",
    )
    .unwrap();
    let s = open(&path);
    assert_eq!(dump(&s), *models.last().unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(!dir.path().join("wal.log.compact.tmp").exists());
}
