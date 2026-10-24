//! **A file by its path, wherever an application runs**: the disk natively,
//! and in a page the origin's own storage (OPFS).
//!
//! An editing application reads its takes, writes its renders and keeps its
//! peaks in files, and the same code runs natively and in a page. Natively
//! every call here is `std::fs`'s. In a page there is no `std::fs` at all, and
//! the origin's storage answers a synchronous read or write only to a worker
//! holding a file's sync access handle, which it gets asynchronously. So in a
//! page a call is a request the calling thread writes into the shared memory
//! and waits on, while a file worker of the page does the work with its sync
//! handles and wakes it ([`set_bridge`], registered by the host's
//! applications' build). Waiting is a thread's: a page's main thread cannot
//! wait, and a call from it is refused rather than made.
//!
//! The surface is the part of `std::fs` the applications use, with its
//! names: [`File`] (`open`, `create`, `Read`, `Write`, `Seek`), [`read`],
//! [`write`], [`rename`], [`remove_file`], [`remove_dir`],
//! [`remove_dir_all`], [`create_dir`], [`create_dir_all`], [`metadata`] and
//! [`read_dir`].

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// **What is known of a file or a directory**: its length in bytes, when it
/// was last modified (milliseconds since the epoch, where the platform says),
/// and whether it is a directory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metadata {
    pub len: u64,
    pub modified_ms: Option<f64>,
    pub is_dir: bool,
}

/// One entry of a directory: its name, and whether it is a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;

    /// An open file: `std::fs::File`.
    #[derive(Debug)]
    pub struct File(pub(super) std::fs::File);

    pub fn open(path: &Path) -> io::Result<File> {
        std::fs::File::open(path).map(File)
    }

    pub fn create(path: &Path) -> io::Result<File> {
        std::fs::File::create(path).map(File)
    }

    pub fn len(file: &File) -> io::Result<u64> {
        file.0.metadata().map(|m| m.len())
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    pub fn remove_dir(path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }

    pub fn remove_dir_all(path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn metadata(path: &Path) -> io::Result<Metadata> {
        let m = std::fs::metadata(path)?;
        let modified_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64() * 1000.0);
        Ok(Metadata {
            len: m.len(),
            modified_ms,
            is_dir: m.is_dir(),
        })
    }

    pub fn read_dir(path: &Path) -> io::Result<Vec<Entry>> {
        std::fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(Entry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_dir: entry.file_type()?.is_dir(),
                })
            })
            .collect()
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// **One call into the page's storage**, as it sits in the shared memory
    /// for the file worker to read and answer: the calling thread fills it,
    /// hands its address to the bridge and waits on `state` until the worker
    /// sets it. The layout is the file worker's to read
    /// (`clients/web/src/gui/apps-files.ts`): `repr(C)`, every field four or
    /// eight bytes, in this order.
    #[repr(C)]
    #[derive(Debug)]
    pub struct Request {
        /// [`PENDING`] until the worker answers, then [`DONE`].
        pub state: AtomicI32,
        /// What to do ([`Op`]).
        pub op: u32,
        /// The file it is done to, for a call on an open one.
        pub handle: u32,
        /// How many bytes a read or a write moves.
        pub len: u32,
        /// Where they are in the memory.
        pub buf: u32,
        /// The path, UTF-8, and its length; then a rename's target.
        pub path: u32,
        pub path_len: u32,
        pub path2: u32,
        pub path2_len: u32,
        /// Where in the file a read or a write starts.
        pub at: f64,
        /// What came back: bytes moved, a handle, a length; negative when it
        /// failed, with the reason in `error`.
        pub result: f64,
        /// A metadata's modification time, or a negative number.
        pub modified: f64,
        /// A metadata's kind: 1 for a directory.
        pub is_dir: u32,
        /// Why it failed, UTF-8, written into `error` up to `error_cap`.
        pub error: u32,
        pub error_cap: u32,
        pub error_len: u32,
    }

    /// A request not answered yet.
    pub const PENDING: i32 = 0;
    /// A request answered.
    pub const DONE: i32 = 1;

    /// What a [`Request`] asks of the page's storage.
    #[repr(u32)]
    #[derive(Clone, Copy, Debug)]
    pub enum Op {
        Open = 1,
        Create = 2,
        Read = 3,
        Write = 4,
        Len = 5,
        Close = 6,
        Rename = 7,
        RemoveFile = 8,
        RemoveDir = 9,
        RemoveDirAll = 10,
        CreateDir = 11,
        CreateDirAll = 12,
        Metadata = 13,
        ReadDir = 14,
    }

    /// **What carries a request to the page's storage and waits for its
    /// answer**: the host's ([`set_bridge`]). Called on a worker; it answers
    /// an error itself on a thread that cannot wait.
    pub type Bridge = fn(&Request) -> io::Result<()>;

    static BRIDGE: OnceLock<Bridge> = OnceLock::new();

    /// **How a page's threads reach its storage**, from now on: the host's
    /// applications' build registers it. The first one stays.
    pub fn set_bridge(bridge: Bridge) {
        let _ = BRIDGE.set(bridge);
    }

    /// An open file of the page's storage: the file worker's handle on it,
    /// and where this one reads and writes next.
    #[derive(Debug)]
    pub struct File {
        handle: u32,
        pub(super) position: u64,
    }

    impl Drop for File {
        fn drop(&mut self) {
            let _ = call(Op::Close, |r| r.handle = self.handle);
        }
    }

    /// One request made and answered; the answer's `result` when it is not
    /// negative, its reason otherwise.
    fn call(op: Op, fill: impl FnOnce(&mut Request)) -> io::Result<Request> {
        let bridge = BRIDGE.get().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "this page has no file storage")
        })?;
        let mut error = vec![0u8; 512];
        let mut request = Request {
            state: AtomicI32::new(PENDING),
            op: op as u32,
            handle: 0,
            len: 0,
            buf: 0,
            path: 0,
            path_len: 0,
            path2: 0,
            path2_len: 0,
            at: 0.0,
            result: 0.0,
            modified: -1.0,
            is_dir: 0,
            error: error.as_mut_ptr() as u32,
            error_cap: error.len() as u32,
            error_len: 0,
        };
        fill(&mut request);
        bridge(&request)?;
        if request.state.load(Ordering::Acquire) != DONE {
            return Err(io::Error::other("the page's storage did not answer"));
        }
        if request.result < 0.0 {
            let len = (request.error_len as usize).min(error.len());
            let reason = String::from_utf8_lossy(&error[..len]).into_owned();
            let kind = match request.result as i64 {
                -2 => io::ErrorKind::NotFound,
                -3 => io::ErrorKind::AlreadyExists,
                -4 => io::ErrorKind::DirectoryNotEmpty,
                _ => io::ErrorKind::Other,
            };
            return Err(io::Error::new(kind, reason));
        }
        Ok(request)
    }

    /// The path as the page's storage names it: `/`-separated, from its root.
    fn named(path: &Path) -> String {
        path.to_string_lossy().trim_start_matches('/').to_string()
    }

    /// `op` on `path`, the path held in place for the call.
    fn on_path(op: Op, path: &Path, fill: impl FnOnce(&mut Request)) -> io::Result<Request> {
        let name = named(path);
        call(op, |r| {
            r.path = name.as_ptr() as u32;
            r.path_len = name.len() as u32;
            fill(r);
        })
    }

    pub fn open(path: &Path) -> io::Result<File> {
        let answer = on_path(Op::Open, path, |_| {})?;
        Ok(File {
            handle: answer.result as u32,
            position: 0,
        })
    }

    pub fn create(path: &Path) -> io::Result<File> {
        let answer = on_path(Op::Create, path, |_| {})?;
        Ok(File {
            handle: answer.result as u32,
            position: 0,
        })
    }

    pub fn len(file: &File) -> io::Result<u64> {
        Ok(call(Op::Len, |r| r.handle = file.handle)?.result as u64)
    }

    pub fn read_at(file: &File, at: u64, buf: &mut [u8]) -> io::Result<usize> {
        let answer = call(Op::Read, |r| {
            r.handle = file.handle;
            r.at = at as f64;
            r.buf = buf.as_mut_ptr() as u32;
            r.len = buf.len() as u32;
        })?;
        Ok(answer.result as usize)
    }

    pub fn write_at(file: &File, at: u64, buf: &[u8]) -> io::Result<usize> {
        let answer = call(Op::Write, |r| {
            r.handle = file.handle;
            r.at = at as f64;
            r.buf = buf.as_ptr() as u32;
            r.len = buf.len() as u32;
        })?;
        Ok(answer.result as usize)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let target = named(to);
        on_path(Op::Rename, from, |r| {
            r.path2 = target.as_ptr() as u32;
            r.path2_len = target.len() as u32;
        })
        .map(drop)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        on_path(Op::RemoveFile, path, |_| {}).map(drop)
    }

    pub fn remove_dir(path: &Path) -> io::Result<()> {
        on_path(Op::RemoveDir, path, |_| {}).map(drop)
    }

    pub fn remove_dir_all(path: &Path) -> io::Result<()> {
        on_path(Op::RemoveDirAll, path, |_| {}).map(drop)
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        on_path(Op::CreateDir, path, |_| {}).map(drop)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        on_path(Op::CreateDirAll, path, |_| {}).map(drop)
    }

    pub fn metadata(path: &Path) -> io::Result<Metadata> {
        let answer = on_path(Op::Metadata, path, |_| {})?;
        Ok(Metadata {
            len: answer.result as u64,
            modified_ms: (answer.modified >= 0.0).then_some(answer.modified),
            is_dir: answer.is_dir == 1,
        })
    }

    /// The entries of a directory, which the file worker writes as lines,
    /// each a name after `d ` or `f `: asked again with room for all of them
    /// when they did not fit.
    pub fn read_dir(path: &Path) -> io::Result<Vec<Entry>> {
        let mut room = 1 << 14;
        loop {
            let mut buf = vec![0u8; room];
            let answer = on_path(Op::ReadDir, path, |r| {
                r.buf = buf.as_mut_ptr() as u32;
                r.len = buf.len() as u32;
            })?;
            let needed = answer.result as usize;
            if needed > room {
                room = needed;
                continue;
            }
            return Ok(String::from_utf8_lossy(&buf[..needed])
                .lines()
                .filter_map(|line| {
                    let (kind, name) = line.split_at_checked(2)?;
                    Some(Entry {
                        name: name.to_string(),
                        is_dir: kind == "d ",
                    })
                })
                .collect());
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::{Bridge, DONE, Op, PENDING, Request, set_bridge};

/// **An open file**, read, written and sought as a `std::fs::File` is.
#[derive(Debug)]
pub struct File(imp::File);

impl File {
    /// Opens the file at `path` to read.
    pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
        imp::open(path.as_ref()).map(File)
    }

    /// Creates the file at `path` to write, emptying one that is there.
    pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
        imp::create(path.as_ref()).map(File)
    }

    /// Its length, in bytes.
    pub fn len(&self) -> io::Result<u64> {
        imp::len(&self.0)
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> io::Result<bool> {
        self.len().map(|len| len == 0)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.0.read(buf)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.0.flush()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.0.seek(pos)
    }
}

#[cfg(target_arch = "wasm32")]
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = imp::read_at(&self.0, self.0.position, buf)?;
        self.0.position += n as u64;
        Ok(n)
    }
}

#[cfg(target_arch = "wasm32")]
impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = imp::write_at(&self.0, self.0.position, buf)?;
        self.0.position += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        // every write is the storage's as it is made
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::Current(by) => self.0.position.checked_add_signed(by),
            SeekFrom::End(by) => self.len()?.checked_add_signed(by),
        };
        self.0.position =
            to.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before 0"))?;
        Ok(self.0.position)
    }
}

/// The whole of the file at `path`.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(file.len().unwrap_or(0) as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Writes `bytes` as the file at `path`, replacing what was there.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    File::create(path)?.write_all(bytes.as_ref())
}

/// Moves the file at `from` to `to`, over whatever is there.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    imp::rename(from.as_ref(), to.as_ref())
}

/// Removes the file at `path`.
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    imp::remove_file(path.as_ref())
}

/// Removes the empty directory at `path`.
pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    imp::remove_dir(path.as_ref())
}

/// Removes the directory at `path` and what it holds.
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    imp::remove_dir_all(path.as_ref())
}

/// Makes the directory at `path`; the one above must be there.
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    imp::create_dir(path.as_ref())
}

/// Makes the directory at `path` and every one above it.
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    imp::create_dir_all(path.as_ref())
}

/// What is known of the file or directory at `path`.
pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    imp::metadata(path.as_ref())
}

/// Whether anything is at `path`.
pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

/// The entries of the directory at `path`.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<Vec<Entry>> {
    imp::read_dir(path.as_ref())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_written_read_sought_and_moved() {
        let dir = std::env::temp_dir().join(format!("clausters-files-{}", std::process::id()));
        let _ = remove_dir_all(&dir);
        create_dir_all(dir.join("a/b")).unwrap();
        let path = dir.join("a/b/take.bin");
        write(&path, b"0123456789").unwrap();
        let mut file = File::open(&path).unwrap();
        assert_eq!(file.len().unwrap(), 10);
        file.seek(SeekFrom::Start(4)).unwrap();
        let mut four = [0u8; 4];
        file.read_exact(&mut four).unwrap();
        assert_eq!(&four, b"4567");
        drop(file);
        let meta = metadata(&path).unwrap();
        assert_eq!((meta.len, meta.is_dir), (10, false));
        assert!(meta.modified_ms.is_some());
        rename(&path, dir.join("a/moved.bin")).unwrap();
        assert!(!exists(&path));
        let mut names: Vec<Entry> = read_dir(dir.join("a")).unwrap();
        names.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(
            names,
            vec![
                Entry {
                    name: "b".into(),
                    is_dir: true
                },
                Entry {
                    name: "moved.bin".into(),
                    is_dir: false
                }
            ]
        );
        assert_eq!(read(dir.join("a/moved.bin")).unwrap(), b"0123456789");
        remove_file(dir.join("a/moved.bin")).unwrap();
        remove_dir(dir.join("a/b")).unwrap();
        remove_dir_all(&dir).unwrap();
        assert!(!exists(&dir));
    }
}
