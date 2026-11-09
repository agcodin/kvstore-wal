# Write path and recovery path

## Write path (`put` and `delete`)

```mermaid
flowchart TD
    A[put key, value] --> B[encode record: len, crc32, payload]
    B --> C[storage.append]
    C --> D{SyncPolicy::Always?}
    D -- yes --> E[storage.sync / fsync]
    D -- no --> F
    E --> F{append or sync failed?}
    F -- no --> G[log_len += record size]
    G --> H[apply to in-memory index]
    H --> I{log over threshold and over 2x live size?}
    I -- yes --> J[compact]
    I -- no --> K[return Ok]
    J --> K
    F -- yes --> L[truncate log back to old log_len, then sync]
    L --> M{rollback worked?}
    M -- yes --> N[return Err, store still usable]
    M -- no --> O[mark store poisoned, return Err]
```

The index is updated only after the record is confirmed. The rollback on failure is the fix described in [writeup.md](writeup.md). Without it, a partial record could stay in the log and hide every later write from recovery.

## Recovery path (`Store::open`)

```mermaid
flowchart TD
    A[read whole log] --> B[pos = 0]
    B --> C{pos at end of log?}
    C -- yes --> H
    C -- no --> D[decode record at pos]
    D --> E{valid record?}
    E -- yes --> F[apply to index, pos += record size]
    F --> C
    E -- "incomplete or bad crc or bad length" --> G[stop]
    G --> H{bytes left after pos?}
    H -- yes --> I[truncate log to pos, then sync]
    H -- no --> J[ready]
    I --> J
```

Truncating before accepting new writes matters. If the damaged tail stayed in the file, the next acknowledged write would land behind it and be dropped on the following restart.

## Compaction

`compact` encodes one put per live key into a buffer and calls `storage.replace`. The file implementation writes a temp file, fsyncs it, renames it over the log and fsyncs the directory. A crash leaves either the old log or the new one. A stale temp file from a crash before the rename is deleted on the next open.
