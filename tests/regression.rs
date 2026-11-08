//! Regression tests for bugs found by the soak harness.

use kvstore_wal::sim::SimDisk;
use kvstore_wal::{Options, Store, SyncPolicy};

fn opts() -> Options {
    Options {
        sync: SyncPolicy::Always,
        auto_compact_min_bytes: None,
    }
}

/// A write that fails partway leaves a partial record in the log. The store
/// must not let later acknowledged writes land behind it, where recovery
/// would stop and drop them.
#[test]
fn failed_append_does_not_hide_later_writes() {
    for seed in 0..64 {
        let disk = SimDisk::new(seed);
        let mut s = Store::open_with(disk.clone(), opts()).unwrap();
        s.put(b"before", b"1").unwrap();

        disk.fail_in(0); // the next storage call, the append, fails midway
        assert!(s.put(b"failed", b"x").is_err());
        s.put(b"after", b"2").unwrap();
        drop(s);

        disk.power_cycle();
        let s = Store::open_with(disk, opts()).unwrap();
        assert_eq!(s.get(b"before"), Some(&b"1"[..]), "seed {seed}");
        assert_eq!(s.get(b"after"), Some(&b"2"[..]), "seed {seed}");
        assert_eq!(s.get(b"failed"), None, "seed {seed}");
    }
}

/// An fsync error after a complete append must not leave the record to be
/// made durable by the next successful fsync.
#[test]
fn failed_sync_does_not_resurrect_write() {
    let disk = SimDisk::new(1);
    let mut s = Store::open_with(disk.clone(), opts()).unwrap();
    disk.fail_in(1); // append ok, sync fails
    assert!(s.put(b"failed", b"x").is_err());
    s.put(b"after", b"2").unwrap();
    drop(s);
    disk.power_cycle();
    let s = Store::open_with(disk, opts()).unwrap();
    assert_eq!(s.get(b"failed"), None);
    assert_eq!(s.get(b"after"), Some(&b"2"[..]));
}
