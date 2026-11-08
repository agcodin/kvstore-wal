use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use kvstore_wal::{FileStorage, Options, Store, SyncPolicy};
use std::path::Path;

const VALUE: [u8; 100] = [7u8; 100];

fn open(path: &Path, sync: SyncPolicy) -> Store {
    let o = Options {
        sync,
        auto_compact_min_bytes: None,
    };
    Store::open_with(FileStorage::open(path).unwrap(), o).unwrap()
}

fn key(i: u64) -> [u8; 8] {
    i.to_be_bytes()
}

fn fill(path: &Path, n: u64) {
    let mut s = open(path, SyncPolicy::Manual);
    for i in 0..n {
        s.put(&key(i), &VALUE).unwrap();
    }
    s.sync().unwrap();
}

fn put(c: &mut Criterion) {
    let mut g = c.benchmark_group("put_100B");
    g.throughput(Throughput::Elements(1));
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir.path().join("a.log"), SyncPolicy::Manual);
    let mut i = 0u64;
    g.bench_function("no_fsync", |b| {
        b.iter(|| {
            s.put(&key(i % 100_000), &VALUE).unwrap();
            i += 1;
        })
    });
    let mut s = open(&dir.path().join("b.log"), SyncPolicy::Always);
    g.sample_size(10);
    g.bench_function("fsync_each", |b| {
        b.iter(|| {
            s.put(&key(i % 100_000), &VALUE).unwrap();
            i += 1;
        })
    });
    g.finish();
}

fn get(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.log");
    fill(&path, 100_000);
    let s = open(&path, SyncPolicy::Manual);
    let mut i = 0u64;
    let mut g = c.benchmark_group("get");
    g.throughput(Throughput::Elements(1));
    g.bench_function("hit_100k_keys", |b| {
        b.iter(|| {
            i = (i + 7919) % 100_000;
            black_box(s.get(&key(i)));
        })
    });
    g.finish();
}

fn recovery(c: &mut Criterion) {
    let mut g = c.benchmark_group("recovery");
    g.sample_size(10);
    for n in [1_000u64, 10_000, 100_000, 1_000_000] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.log");
        fill(&path, n);
        let bytes = std::fs::metadata(&path).unwrap().len();
        eprintln!("recovery log: {n} records, {bytes} bytes");
        g.throughput(Throughput::Elements(n));
        g.bench_with_input(BenchmarkId::new("open_records", n), &n, |b, _| {
            b.iter(|| black_box(open(&path, SyncPolicy::Manual)))
        });
    }
    g.finish();
}

fn compaction(c: &mut Criterion) {
    let mut g = c.benchmark_group("compaction");
    g.sample_size(10);
    // 100k live keys, each overwritten 5 times: log is ~5x the live data.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.log");
    {
        let mut s = open(&path, SyncPolicy::Manual);
        for round in 0..5 {
            for i in 0..100_000u64 {
                s.put(&key(i), &[round as u8; 100]).unwrap();
            }
        }
        s.sync().unwrap();
    }
    let before = std::fs::metadata(&path).unwrap().len();
    eprintln!("compaction input log: {before} bytes");
    g.bench_function("100k_keys_5x_overwritten", |b| {
        b.iter_batched(
            || {
                let d = tempfile::tempdir().unwrap();
                let p = d.path().join("c.log");
                std::fs::copy(&path, &p).unwrap();
                (open(&p, SyncPolicy::Manual), d)
            },
            |(mut s, d)| {
                s.compact().unwrap();
                black_box(s.log_len());
                d
            },
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

criterion_group!(benches, put, get, recovery, compaction);
criterion_main!(benches);
