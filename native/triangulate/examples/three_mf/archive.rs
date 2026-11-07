use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use zip::ZipArchive;

pub(super) type Archive = ZipArchive<PositionedFile>;

// ZIP readers share the directory and file handle, but each decoder has its own cursor.
#[derive(Clone)]
pub(super) struct PositionedFile {
    file: Arc<File>,
    len: u64,
    position: u64,
}

impl Read for PositionedFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let read = self.file.read_at(output, self.position)?;
        #[cfg(windows)]
        let read = self.file.seek_read(output, self.position)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for PositionedFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid ZIP position"))?;
        Ok(self.position)
    }
}

pub(super) fn open(path: &Path) -> super::Result<Archive> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok(ZipArchive::new(PositionedFile {
        file: Arc::new(file),
        len,
        position: 0,
    })?)
}
