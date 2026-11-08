use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The minimal file operations the store needs. Keeping this small lets the
/// soak test swap in a simulated disk that can crash and fail on demand.
pub trait Storage {
    /// Everything currently in the log, including bytes not yet synced.
    fn read_all(&mut self) -> io::Result<Vec<u8>>;
    /// Append at the end of the log. May write a prefix of `data` and then fail.
    fn append(&mut self, data: &[u8]) -> io::Result<()>;
    /// Make everything appended so far durable.
    fn sync(&mut self) -> io::Result<()>;
    /// Cut the log to `len` bytes.
    fn truncate(&mut self, len: u64) -> io::Result<()>;
    /// Atomically and durably replace the whole log with `data`. On failure the
    /// old contents must still be intact.
    fn replace(&mut self, data: &[u8]) -> io::Result<()>;
}

pub struct FileStorage {
    path: PathBuf,
    file: File,
}

impl FileStorage {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        // A compaction that crashed before its rename leaves this behind.
        let _ = fs::remove_file(tmp_path(&path));
        let file = open_log(&path)?;
        Ok(Self { path, file })
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".compact.tmp");
    PathBuf::from(s)
}

fn open_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
}

impl Storage for FileStorage {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut v = Vec::new();
        self.file.read_to_end(&mut v)?;
        Ok(v)
    }

    fn append(&mut self, data: &[u8]) -> io::Result<()> {
        self.file.write_all(data)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }

    fn replace(&mut self, data: &[u8]) -> io::Result<()> {
        let tmp = tmp_path(&self.path);
        {
            let mut f = File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        if let Some(dir) = self.path.parent() {
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            File::open(dir)?.sync_all()?;
        }
        self.file = open_log(&self.path)?;
        Ok(())
    }
}
