use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::format::{End, Event, Header, Record, Sample};

/// Writes a recording with user-only permissions and a size limit.
pub struct RecordingWriter {
    out: BufWriter<File>,
    bytes: u64,
    max_bytes: u64,
    pub samples: u64,
    pub dropped: u64,
    limit_hit: bool,
}

pub enum WriteOutcome {
    Written,
    /// The size limit was reached; the caller should stop and finish.
    LimitReached,
}

impl RecordingWriter {
    /// Creates `path` (refusing to overwrite unless `overwrite`).
    pub fn create(
        path: &Path,
        header: &Header,
        max_bytes: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        let mut opts = OpenOptions::new();
        opts.write(true);
        if overwrite {
            opts.create(true).truncate(true);
        } else {
            opts.create_new(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path)?;
        let mut w = RecordingWriter {
            out: BufWriter::new(file),
            bytes: 0,
            max_bytes,
            samples: 0,
            dropped: 0,
            limit_hit: false,
        };
        w.write(&Record::Header(Box::new(header.clone())))?;
        w.out.flush()?;
        Ok(w)
    }

    fn write(&mut self, r: &Record) -> io::Result<()> {
        let mut line = serde_json::to_vec(r).map_err(io::Error::other)?;
        line.push(b'\n');
        self.out.write_all(&line)?;
        self.bytes += line.len() as u64;
        Ok(())
    }

    pub fn sample(&mut self, s: Sample) -> io::Result<WriteOutcome> {
        if self.limit_hit {
            self.dropped += 1;
            return Ok(WriteOutcome::LimitReached);
        }
        // Reserve room for the end record (~1 KiB).
        let line = serde_json::to_vec(&Record::Sample(Box::new(s))).map_err(io::Error::other)?;
        if self.bytes + line.len() as u64 + 1024 > self.max_bytes {
            self.limit_hit = true;
            self.dropped += 1;
            return Ok(WriteOutcome::LimitReached);
        }
        self.out.write_all(&line)?;
        self.out.write_all(b"\n")?;
        self.bytes += line.len() as u64 + 1;
        self.samples += 1;
        // Flush per sample so an interrupted recording keeps its data.
        self.out.flush()?;
        Ok(WriteOutcome::Written)
    }

    pub fn event(&mut self, e: Event) -> io::Result<()> {
        self.write(&Record::Event(e))?;
        self.out.flush()
    }

    pub fn limit_hit(&self) -> bool {
        self.limit_hit
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn finish(mut self, mut end: End) -> io::Result<u64> {
        end.samples = self.samples;
        end.dropped_samples = self.dropped;
        end.truncated |= self.limit_hit;
        self.write(&Record::End(end))?;
        self.out.flush()?;
        self.out.get_ref().sync_all()?;
        Ok(self.bytes)
    }
}
