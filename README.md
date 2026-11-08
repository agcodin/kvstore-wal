# kvstore-wal

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

Limits: single writer behind a mutex, whole log read into memory on open and on compaction, values held in memory, keys in the TCP protocol cannot contain spaces.
