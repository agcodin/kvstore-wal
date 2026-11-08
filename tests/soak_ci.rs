use kvstore_wal::soak;

#[test]
fn soak_smoke() {
    let bad: Vec<_> = (0..2000u64)
        .filter_map(|s| soak::run_one(s).err().map(|e| (s, e)))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}
