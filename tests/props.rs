//! Property tests: the store must behave like a BTreeMap, including across
//! reopen and compaction.

use kvstore_wal::sim::SimDisk;
use kvstore_wal::{Options, Store, SyncPolicy};
use proptest::prelude::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
enum Cmd {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
    Get(Vec<u8>),
    Scan(Vec<u8>),
    Compact,
    Reopen,
}

fn key() -> impl Strategy<Value = Vec<u8>> {
    // Small alphabet and length so prefixes and collisions are common.
    proptest::collection::vec(prop::sample::select(vec![b'a', b'b', b'c']), 0..4)
}

fn cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        4 => (key(), proptest::collection::vec(any::<u8>(), 0..32)).prop_map(|(k, v)| Cmd::Put(k, v)),
        2 => key().prop_map(Cmd::Delete),
        2 => key().prop_map(Cmd::Get),
        2 => key().prop_map(Cmd::Scan),
        1 => Just(Cmd::Compact),
        1 => Just(Cmd::Reopen),
    ]
}

fn opts() -> Options {
    Options {
        sync: SyncPolicy::Always,
        auto_compact_min_bytes: Some(200),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn matches_btreemap(cmds in proptest::collection::vec(cmd(), 0..120)) {
        let disk = SimDisk::new(7);
        let mut store = Store::open_with(disk.clone(), opts()).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for c in cmds {
            match c {
                Cmd::Put(k, v) => {
                    store.put(&k, &v).unwrap();
                    model.insert(k, v);
                }
                Cmd::Delete(k) => {
                    prop_assert_eq!(store.delete(&k).unwrap(), model.remove(&k).is_some());
                }
                Cmd::Get(k) => {
                    prop_assert_eq!(store.get(&k), model.get(&k).map(|v| v.as_slice()));
                }
                Cmd::Scan(p) => {
                    let got: Vec<_> = store.scan_prefix(&p).map(|(k, v)| (k.to_vec(), v.to_vec())).collect();
                    let want: Vec<_> = model.iter().filter(|(k, _)| k.starts_with(&p)).map(|(k, v)| (k.clone(), v.clone())).collect();
                    prop_assert_eq!(got, want);
                }
                Cmd::Compact => store.compact().unwrap(),
                Cmd::Reopen => {
                    drop(store);
                    store = Store::open_with(disk.clone(), opts()).unwrap();
                }
            }
            prop_assert_eq!(store.len(), model.len());
        }
        drop(store);
        let store = Store::open_with(disk, opts()).unwrap();
        let got: BTreeMap<_, _> = store.scan_prefix(b"").map(|(k, v)| (k.to_vec(), v.to_vec())).collect();
        prop_assert_eq!(got, model);
    }
}
