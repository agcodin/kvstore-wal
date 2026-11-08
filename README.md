# kvstore-wal

[![ci](https://github.com/agcodin/kvstore-wal/actions/workflows/ci.yml/badge.svg)](https://github.com/agcodin/kvstore-wal/actions/workflows/ci.yml)

A small durable key-value store in Rust. An ordered in-memory index sits over an append-only write-ahead log. Records are length-prefixed and CRC-32 checked, and recovery stops cleanly at a torn or corrupt record.

- `put`, `get`, `delete`, `scan_prefix`, `compact`
- `SyncPolicy::Always` (fsync per write) or `Manual`
- Compaction rewrites the log to one put per live key via temp file and atomic rename, and runs automatically when the log is over twice the live size
- `kvd` is a line-based TCP server (`PUT k v`, `GET k`, `DEL k`, `SCAN prefix`, `COMPACT`)

```sh
cargo test                                    # unit, proptest, crash sweeps, regression, TCP
cargo run --release --bin soak -- --iterations 20000   # randomized crash/fault soak
cargo bench                                   # criterion
cargo run --release --bin kvd -- --dir data   # server on 127.0.0.1:7878
docker build -t kvstore-wal . && docker run -p 7878:7878 -v kv:/data kvstore-wal
```

Layout: `src/record.rs` (format), `src/store.rs` (store), `src/storage.rs` (file I/O), `src/sim.rs` (simulated disk), `src/soak.rs` (soak scenario). See `docs/writeup.md` for the bug the soak found, and `docs/` for raw soak and benchmark output.

## Measured

Criterion on an Apple M5 Pro, 100-byte values, one run each (`docs/bench-raw.txt`, `docs/bench-sizes.txt`).

| Benchmark | Time (median of estimate) |
|---|---|
| put, no fsync | 2.11 µs |
| put, fsync per write | 3.07 ms |
| get, 100k keys | 154 ns |
| recovery, 1,000 records (121 KB) | 148 µs |
| recovery, 10,000 records (1.21 MB) | 1.44 ms |
| recovery, 100,000 records (12.1 MB) | 16.4 ms |
| recovery, 1,000,000 records (121 MB) | 177 ms |
| compact 60.5 MB log, 100k live keys | 20.8 ms |

Soak results are in `docs/soak-before-fix.txt` (7,566 of 10,000 seeds failed before the fix), `docs/soak-after-fix.txt` and `docs/final-run.txt` (0 failures in 10,000, 20,000 and 200,000 seeds after it). Each bug the soak exposed has a test in `tests/regression.rs`.

## Limits

Single writer behind a mutex, whole log read into memory on open and on compaction, values held in memory, keys in the TCP protocol cannot contain spaces. Recovery stops at the first bad record, so damage in the middle of the log discards every newer write (the count is reported). The soak uses a simulated disk that models partial writes and lost or garbled unsynced tails, not reordered writes or a lost directory sync. The CI workflow and the badge URL have not been exercised on GitHub.
