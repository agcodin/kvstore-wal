use crate::record::{self, Decoded, Op};
use crate::storage::{FileStorage, Storage};
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Key or value too large for one record.
    TooLarge,
    /// A failed write could not be rolled back, so the log may hold a partial
    /// record. Writes are refused until the store is reopened and recovery
    /// cuts the log back to its last valid record.
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::TooLarge => write!(f, "record too large"),
            Error::Poisoned => write!(f, "store poisoned by a failed write; reopen it"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// fsync after every write. A returned `Ok` means the write is durable.
    Always,
    /// Never fsync on write. Call [`Store::sync`] yourself.
    Manual,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub sync: SyncPolicy,
    /// Compact automatically when the log is at least this big and more than
    /// twice the size of the live data. `None` disables it.
    pub auto_compact_min_bytes: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sync: SyncPolicy::Always,
            auto_compact_min_bytes: Some(4 << 20),
        }
    }
}

/// What recovery found when the log was opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    pub records: usize,
    pub valid_bytes: u64,
    /// Bytes after the last valid record that were cut off.
    pub discarded_bytes: u64,
}

pub struct Store<S: Storage = FileStorage> {
    storage: S,
    index: BTreeMap<Vec<u8>, Vec<u8>>,
    opts: Options,
    log_len: u64,
    live_bytes: u64,
    recovery: Recovery,
    poisoned: bool,
}

impl Store<FileStorage> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(FileStorage::open(path)?, Options::default())
    }
}

impl<S: Storage> Store<S> {
    pub fn open_with(mut storage: S, opts: Options) -> Result<Self> {
        let data = storage.read_all()?;
        let mut index = BTreeMap::new();
        let mut live_bytes = 0u64;
        let mut pos = 0usize;
        let mut records = 0usize;
        while pos < data.len() {
            match record::decode(&data[pos..]) {
                Decoded::Record(op, n) => {
                    apply(&mut index, &mut live_bytes, op);
                    pos += n;
                    records += 1;
                }
                Decoded::Incomplete | Decoded::Corrupt => break,
            }
        }
        let discarded = (data.len() - pos) as u64;
        if discarded > 0 {
            storage.truncate(pos as u64)?;
            storage.sync()?;
        }
        Ok(Store {
            storage,
            index,
            opts,
            log_len: pos as u64,
            live_bytes,
            poisoned: false,
            recovery: Recovery {
                records,
                valid_bytes: pos as u64,
                discarded_bytes: discarded,
            },
        })
    }

    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.index.get(key).map(|v| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn log_len(&self) -> u64 {
        self.log_len
    }

    /// Entries whose key starts with `prefix`, in key order.
    pub fn scan_prefix<'a>(
        &'a self,
        prefix: &'a [u8],
    ) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + 'a {
        self.index
            .range::<[u8], _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if 1 + 4 + key.len() + value.len() > record::MAX_PAYLOAD {
            return Err(Error::TooLarge);
        }
        let op = Op::Put {
            key: key.to_vec(),
            value: value.to_vec(),
        };
        self.append(&op)?;
        apply(&mut self.index, &mut self.live_bytes, op);
        self.maybe_compact();
        Ok(())
    }

    /// Returns whether the key existed. Deleting a missing key writes nothing.
    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        if !self.index.contains_key(key) {
            return Ok(false);
        }
        let op = Op::Delete { key: key.to_vec() };
        self.append(&op)?;
        apply(&mut self.index, &mut self.live_bytes, op);
        self.maybe_compact();
        Ok(true)
    }

    pub fn sync(&mut self) -> Result<()> {
        self.storage.sync()?;
        Ok(())
    }

    /// Rewrite the log as one put per live key.
    pub fn compact(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let mut buf = Vec::with_capacity(self.live_bytes as usize);
        for (k, v) in &self.index {
            record::encode(
                &Op::Put {
                    key: k.clone(),
                    value: v.clone(),
                },
                &mut buf,
            );
        }
        self.storage.replace(&buf)?;
        self.log_len = buf.len() as u64;
        Ok(())
    }

    fn append(&mut self, op: &Op) -> Result<()> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let mut buf = Vec::new();
        record::encode(op, &mut buf);
        let written = self.storage.append(&buf).and_then(|()| {
            if self.opts.sync == SyncPolicy::Always {
                self.storage.sync()
            } else {
                Ok(())
            }
        });
        if let Err(e) = written {
            // The log may now end in a partial record, or a complete one that
            // was never confirmed. Anything appended after it would sit behind
            // bytes that recovery stops at, or would revive this failed write.
            // Cut back to the last confirmed length before accepting more.
            let rolled_back = self
                .storage
                .truncate(self.log_len)
                .and_then(|()| self.storage.sync());
            if rolled_back.is_err() {
                self.poisoned = true;
            }
            return Err(e.into());
        }
        self.log_len += buf.len() as u64;
        Ok(())
    }

    fn maybe_compact(&mut self) {
        if let Some(min) = self.opts.auto_compact_min_bytes {
            if self.log_len >= min && self.log_len > 2 * self.live_bytes {
                // The write that triggered this is already durable, so a failed
                // compaction only costs space.
                let _ = self.compact();
            }
        }
    }
}

fn apply(index: &mut BTreeMap<Vec<u8>, Vec<u8>>, live_bytes: &mut u64, op: Op) {
    match op {
        Op::Put { key, value } => {
            *live_bytes += record::put_len(&key, &value) as u64;
            if let Some(old) = index.insert(key.clone(), value) {
                *live_bytes -= record::put_len(&key, &old) as u64;
            }
        }
        Op::Delete { key } => {
            if let Some(old) = index.remove(&key) {
                *live_bytes -= record::put_len(&key, &old) as u64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimDisk;

    fn opts() -> Options {
        Options {
            sync: SyncPolicy::Always,
            auto_compact_min_bytes: None,
        }
    }

    #[test]
    fn put_get_delete() {
        let mut s = Store::open_with(SimDisk::new(1), opts()).unwrap();
        assert_eq!(s.get(b"a"), None);
        s.put(b"a", b"1").unwrap();
        s.put(b"a", b"2").unwrap();
        assert_eq!(s.get(b"a"), Some(&b"2"[..]));
        assert!(s.delete(b"a").unwrap());
        assert!(!s.delete(b"a").unwrap());
        assert_eq!(s.get(b"a"), None);
    }

    #[test]
    fn scan_prefix_is_ordered_and_bounded() {
        let mut s = Store::open_with(SimDisk::new(1), opts()).unwrap();
        for k in ["ab", "aa", "b", "abc", "a", "ac"] {
            s.put(k.as_bytes(), b"v").unwrap();
        }
        let got: Vec<_> = s.scan_prefix(b"ab").map(|(k, _)| k.to_vec()).collect();
        assert_eq!(got, vec![b"ab".to_vec(), b"abc".to_vec()]);
        assert_eq!(s.scan_prefix(b"").count(), 6);
        assert_eq!(s.scan_prefix(b"zz").count(), 0);
    }

    #[test]
    fn reopen_replays_log() {
        let disk = SimDisk::new(1);
        let mut s = Store::open_with(disk.clone(), opts()).unwrap();
        s.put(b"a", b"1").unwrap();
        s.put(b"b", b"2").unwrap();
        s.delete(b"a").unwrap();
        drop(s);
        let s = Store::open_with(disk, opts()).unwrap();
        assert_eq!(s.get(b"a"), None);
        assert_eq!(s.get(b"b"), Some(&b"2"[..]));
        assert_eq!(s.recovery().records, 3);
        assert_eq!(s.recovery().discarded_bytes, 0);
    }

    #[test]
    fn compaction_shrinks_log_and_preserves_state() {
        let disk = SimDisk::new(1);
        let mut s = Store::open_with(disk.clone(), opts()).unwrap();
        for i in 0..100 {
            s.put(b"hot", format!("v{i}").as_bytes()).unwrap();
        }
        s.put(b"cold", b"x").unwrap();
        let before = s.log_len();
        s.compact().unwrap();
        assert!(s.log_len() < before / 10);
        s.put(b"after", b"y").unwrap();
        drop(s);
        let s = Store::open_with(disk, opts()).unwrap();
        assert_eq!(s.get(b"hot"), Some(&b"v99"[..]));
        assert_eq!(s.get(b"cold"), Some(&b"x"[..]));
        assert_eq!(s.get(b"after"), Some(&b"y"[..]));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn auto_compaction_triggers() {
        let o = Options {
            sync: SyncPolicy::Always,
            auto_compact_min_bytes: Some(512),
        };
        let mut s = Store::open_with(SimDisk::new(1), o).unwrap();
        for i in 0..200 {
            s.put(b"k", format!("value-{i}").as_bytes()).unwrap();
        }
        assert!(s.log_len() < 2048, "log_len={}", s.log_len());
    }

    #[test]
    fn torn_tail_is_cut_and_new_writes_survive() {
        let disk = SimDisk::new(1);
        let mut s = Store::open_with(disk.clone(), opts()).unwrap();
        s.put(b"a", b"1").unwrap();
        drop(s);
        // Append half a record by hand.
        let mut rec = Vec::new();
        record::encode(
            &Op::Put {
                key: b"b".to_vec(),
                value: b"2".to_vec(),
            },
            &mut rec,
        );
        let mut d = disk.clone();
        d.append(&rec[..rec.len() - 3]).unwrap();
        d.sync().unwrap();

        let mut s = Store::open_with(disk.clone(), opts()).unwrap();
        assert_eq!(s.recovery().discarded_bytes as usize, rec.len() - 3);
        assert_eq!(s.get(b"b"), None);
        s.put(b"c", b"3").unwrap();
        drop(s);
        let s = Store::open_with(disk, opts()).unwrap();
        assert_eq!(s.get(b"a"), Some(&b"1"[..]));
        assert_eq!(s.get(b"c"), Some(&b"3"[..]));
    }

    #[test]
    fn oversized_record_rejected() {
        let mut s = Store::open_with(SimDisk::new(1), opts()).unwrap();
        let big = vec![0u8; record::MAX_PAYLOAD];
        assert!(matches!(s.put(b"k", &big), Err(Error::TooLarge)));
    }
}
