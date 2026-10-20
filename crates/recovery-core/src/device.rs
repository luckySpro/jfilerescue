//! Read-only block device abstraction.
//!
//! Everything in the core reads through [`BlockDevice`]. Implementations are
//! plain files / disk images (here) and raw OS devices (in `recovery-platform`).
//! Nothing in this crate ever writes to a source device.

use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

/// A read-only, random-access byte source.
pub trait BlockDevice: Send + Sync {
    /// Total size in bytes.
    fn size(&self) -> u64;
    /// Logical sector size (512 or 4096 typically).
    fn sector_size(&self) -> u32 {
        512
    }
    /// Read exactly `buf.len()` bytes at `offset`. Must return an error (not
    /// short read) when the range is out of bounds.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Human readable identifier for logs/UI.
    fn describe(&self) -> String;

    /// Read a fresh Vec.
    fn read_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        self.read_at(offset, &mut v)?;
        Ok(v)
    }

    /// Best-effort read: on I/O error fills the unread part with zeros and
    /// returns the number of bytes successfully read. Used by the carver so a
    /// single bad sector does not abort a full-disk scan.
    fn read_lossy(&self, offset: u64, buf: &mut [u8]) -> usize {
        if self.read_at(offset, buf).is_ok() {
            return buf.len();
        }
        // Retry per sector so we keep the good parts.
        let ss = self.sector_size() as usize;
        let mut ok = 0usize;
        for (i, chunk) in buf.chunks_mut(ss).enumerate() {
            let off = offset + (i * ss) as u64;
            if self.read_at(off, chunk).is_ok() {
                ok += chunk.len();
            } else {
                chunk.fill(0);
            }
        }
        ok
    }
}

/// A sub-range of another device (a partition / volume inside a disk).
pub struct SliceDevice<D: BlockDevice> {
    inner: D,
    start: u64,
    len: u64,
    name: String,
}

impl<D: BlockDevice> SliceDevice<D> {
    pub fn new(inner: D, start: u64, len: u64, name: impl Into<String>) -> Result<Self> {
        if start
            .checked_add(len)
            .map(|e| e > inner.size())
            .unwrap_or(true)
        {
            return Err(Error::InvalidInput(format!(
                "分区范围 {}..{} 超出设备大小 {}",
                start,
                start + len,
                inner.size()
            )));
        }
        Ok(Self {
            inner,
            start,
            len,
            name: name.into(),
        })
    }
    pub fn inner(&self) -> &D {
        &self.inner
    }
    pub fn start(&self) -> u64 {
        self.start
    }
}

impl<D: BlockDevice> BlockDevice for SliceDevice<D> {
    fn size(&self) -> u64 {
        self.len
    }
    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| Error::InvalidInput("偏移溢出".into()))?;
        if end > self.len {
            return Err(Error::InvalidInput(format!(
                "读取 {}..{} 超出分区大小 {}",
                offset, end, self.len
            )));
        }
        self.inner.read_at(self.start + offset, buf)
    }
    fn describe(&self) -> String {
        format!("{} @ {}+{}", self.name, self.start, self.len)
    }
}

/// Shared reference wrapper so one underlying device can back several slices.
impl<T: BlockDevice + ?Sized> BlockDevice for std::sync::Arc<T> {
    fn size(&self) -> u64 {
        (**self).size()
    }
    fn sector_size(&self) -> u32 {
        (**self).sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// A regular file or raw disk image opened read-only.
pub struct FileDevice {
    file: Mutex<File>,
    size: u64,
    sector: u32,
    name: String,
}

impl FileDevice {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
            sector: 512,
            name: path.display().to_string(),
        })
    }

    /// Wrap an already-open handle whose length may not be reported by
    /// metadata (raw devices). Used by the platform crate.
    pub fn from_file(file: File, size: u64, sector: u32, name: impl Into<String>) -> Self {
        Self {
            file: Mutex::new(file),
            size,
            sector,
            name: name.into(),
        }
    }
}

impl BlockDevice for FileDevice {
    fn size(&self) -> u64 {
        self.size
    }
    fn sector_size(&self) -> u32 {
        self.sector
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| Error::InvalidInput("偏移溢出".into()))?;
        if end > self.size {
            return Err(Error::InvalidInput(format!(
                "读取 {}..{} 超出设备大小 {}",
                offset, end, self.size
            )));
        }
        let mut f = self
            .file
            .lock()
            .map_err(|_| Error::Other("设备锁被毒化".into()))?;
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)?;
        Ok(())
    }
    fn describe(&self) -> String {
        self.name.clone()
    }
}

/// In-memory device for tests.
pub struct MemDevice {
    data: Vec<u8>,
    sector: u32,
}

impl MemDevice {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, sector: 512 }
    }
    pub fn with_sector(data: Vec<u8>, sector: u32) -> Self {
        Self { data, sector }
    }
}

impl BlockDevice for MemDevice {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }
    fn sector_size(&self) -> u32 {
        self.sector
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = offset as usize;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| Error::InvalidInput("偏移溢出".into()))?;
        if end > self.data.len() {
            return Err(Error::InvalidInput(format!(
                "读取 {}..{} 超出设备大小 {}",
                start,
                end,
                self.data.len()
            )));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
    fn describe(&self) -> String {
        format!("mem:{}", self.data.len())
    }
}
