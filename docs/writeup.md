# A failed write that quietly ate the next ones

I built a small key-value store in Rust. It keeps an ordered in-memory index and writes every change to an append-only log first. Each log record has a length, a CRC-32 and a payload, and on startup the store replays the log and stops at the first record that is torn or fails its checksum. Compaction rewrites the log as one put per live key, using a temp file and an atomic rename.

Most of the testing went into one question. If the process dies at any point, or a disk call returns an error, does the store still keep everything it told the caller was saved? This post covers the one real bug that turned up.

## How I tested it

The first layer was ordinary. There are unit tests for the record format, property tests that compare the store against a `BTreeMap` through random puts, deletes, scans, compactions and reopens, and two sweeps over a real log file. The sweeps take a 30-record, 678-byte log, cut it at every byte offset, and flip bytes at every offset with three different masks. Every case has to recover to exactly the state after the last intact record, and a write made after recovery has to survive another restart. All of these passed on the first run.

The second layer is a soak harness. The store only touches its file through a five-method trait (read, append, sync, truncate, replace), so I wrote a simulated disk behind that trait. Appended bytes sit in a pending buffer until a sync moves them to durable storage. A simulated power loss keeps the durable bytes plus a random prefix of the pending ones, and sometimes garbles the last few bytes of that prefix. The disk can also be told to fail one call while staying up, or to die in the middle of a call. A failed append writes a random prefix of the record before it returns the error.

Each soak run does 20 to 200 random operations against a store on that disk, mirrors acknowledged writes in a model, and injects faults and idle power losses along the way. The contract it checks is short. An acknowledged write is never lost. A write that returned an error has no effect. A write interrupted by a crash may or may not be there, but nothing else may differ from the model.

## What it found

My first version of the append path looked like this.

```rust
self.storage.append(&buf)?;
if self.opts.sync == SyncPolicy::Always {
    self.storage.sync()?;
}
self.log_len += buf.len() as u64;
```

I ran 10,000 seeded runs and 7,566 of them reported a violation. That number is high because the fault rate is high, and it lumps together at least two symptoms. The first seed already showed one. After a crash, a key held an older value than the last acknowledged put. Another failure was reported as acknowledged data lost after a power loss.

The common thread, once I looked at what the simulated disk had done, was a failed append. If the disk accepts part of a record and then returns an error, those bytes stay at the end of the log. The store reports the error and carries on, and the next put appends its record after the partial one and syncs successfully. The caller got an acknowledgement. But on the next startup, recovery reads the partial record, fails the length or checksum test, and stops there. Every record behind it is discarded, including the ones that were acknowledged.

I did not trace all 7,566 failures back to this cause by hand. I wrote a deterministic test instead. It makes the next append fail midway, then does a successful put, then power-cycles and reopens. It failed on the first seed I tried, with the later key missing.

Writing that test made me look at the neighbouring case. If the append completes but the sync returns an error, the record is complete and valid in the page cache. The store reports failure, but a later successful sync makes the record durable, so a write the caller was told had failed reappears after a restart. I added a second test for this one. On the old code it failed with the failed key present, where the test expected it to be absent. I found this second case by reasoning about the first, and the soak harness did not report it separately.

## The fix

Both cases come from the same mistake. The store treated an error as if nothing had changed on disk, when the log might have been extended by an unknown amount. The fix is to roll the file back to the last length the store had confirmed.

```rust
if let Err(e) = written {
    let rolled_back = self.storage.truncate(self.log_len)
        .and_then(|()| self.storage.sync());
    if rolled_back.is_err() {
        self.poisoned = true;
    }
    return Err(e.into());
}
```

If the rollback itself fails, the store cannot know what the file holds. It marks itself poisoned and refuses further writes until it is reopened, at which point recovery cuts the log back to the last valid record. The two tests stay in `tests/regression.rs`.

After the change, the same 10,000 seeds reported zero violations, and so did a run of 200,000 seeds that took about 21 seconds. The harness is also part of `cargo test` as a 2,000-seed smoke run, so CI runs it on every push.

## What this does not show

Zero violations in the soak run means the contract holds against my model of a disk. It does not mean the contract holds on a real disk. The simulated disk only produces failures I thought to write down. It models partial writes, lost unsynced tails and garbled tails, and it does not model reordered writes, a rename that survives while the data it points to does not, or a directory entry lost without a directory sync. The real file implementation syncs the parent directory after a rename, but nothing in my tests would notice if it did not.

## Some numbers and a tradeoff

These come from criterion on an Apple M5 Pro, with 100-byte values. A put without fsync took about 2.1 microseconds. A put with an fsync on every write took about 3.07 milliseconds, roughly 1,450 times slower, so durability dominates the cost of a write. Reads from the in-memory index took about 154 nanoseconds. Recovery replayed about 5.6 to 7 million records per second, taking 177 milliseconds for a 1,000,000-record, 121 MB log. Compacting a 60.5 MB log with 100,000 live keys, each overwritten five times, took about 21 milliseconds. Each figure comes from one run on a laptop, so treat them as rough.

The design also has a tradeoff I chose and should state. Recovery stops at the first bad record, whether it is a torn tail or damage in the middle of the log, and it discards everything after it. That is simple and always yields a state the store really passed through, but a single flipped bit in an old record costs every newer write. The store reports how many bytes it discarded, so that loss is visible, and a design that skips past a bad record would need a way to tell a torn tail from real corruption.
