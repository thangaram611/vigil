//! Incremental, bounded JSONL. Only complete records reach reducers.
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const EVALUATION_BYTES: usize = 16 * 1024 * 1024;
const RECORD_BYTES: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub struct Reader {
    pub revision: u64,
    identity: Option<(u64, u64)>,
    offset: u64,
    modified: (i64, i64),
    anchor: Vec<u8>,
    partial: Vec<u8>,
    draining: bool,
    pub gap: bool,
    pub caught_up: bool,
}
impl Reader {
    /// `reset` clears correlated state when a file is replaced or truncated.
    pub fn read(
        &mut self,
        path: &Path,
        budget: &mut usize,
        mut reset: impl FnMut(),
        mut event: impl FnMut(Value),
    ) -> std::io::Result<usize> {
        let mut file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path)?;
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transcript is not a regular file",
            ));
        }
        let id = (m.dev(), m.ino());
        let modified = (m.mtime(), m.mtime_nsec());
        let mut rewritten = false;
        if self.identity == Some(id)
            && m.len() >= self.offset
            && self.modified != modified
            && !self.anchor.is_empty()
        {
            file.seek(SeekFrom::Start(self.offset - self.anchor.len() as u64))?;
            let mut anchor = vec![0; self.anchor.len()];
            file.read_exact(&mut anchor)?;
            rewritten = anchor != self.anchor;
        }
        if self.identity != Some(id)
            || m.len() < self.offset
            || rewritten
            || (m.len() == self.offset && self.modified != modified)
        {
            let revision = self.revision + 1;
            *self = Self::default();
            self.revision = revision;
            self.identity = Some(id);
            reset();
        }
        self.modified = modified;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut consumed = 0;
        let mut chunk = [0u8; 64 * 1024];
        while *budget > 0 {
            let limit = (*budget).min(chunk.len());
            let n = file.read(&mut chunk[..limit])?;
            if n == 0 {
                break;
            }
            self.anchor.extend_from_slice(&chunk[..n]);
            if self.anchor.len() > 64 {
                self.anchor.drain(..self.anchor.len() - 64);
            }
            self.offset += n as u64;
            *budget -= n;
            consumed += n;
            for byte in &chunk[..n] {
                if *byte == b'\n' {
                    if !self.draining && !self.partial.is_empty() {
                        match serde_json::from_slice(&self.partial) {
                            Ok(v) => event(v),
                            Err(_) => self.gap = true,
                        }
                    }
                    self.partial.clear();
                    self.draining = false;
                } else if !self.draining {
                    if self.partial.len() == RECORD_BYTES {
                        self.gap = true;
                        self.draining = true;
                        self.partial.clear();
                    } else {
                        self.partial.push(*byte);
                    }
                }
            }
        }
        self.caught_up =
            self.offset == file.metadata()?.len() && self.partial.is_empty() && !self.draining;
        Ok(consumed)
    }
}
