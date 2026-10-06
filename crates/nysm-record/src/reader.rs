use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use crate::format::{End, Event, FORMAT, FORMAT_VERSION, Header, Record, Sample};

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    NotARecording(String),
    UnsupportedVersion { found: u32, supported: u32 },
    Corrupt { line: usize, error: String },
    TooLarge { limit: u64 },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::NotARecording(why) => write!(f, "not a recording: {why}"),
            ReadError::UnsupportedVersion { found, supported } => {
                write!(
                    f,
                    "recording format version {found} is newer than supported ({supported}); upgrade nysm"
                )
            }
            ReadError::Corrupt { line, error } => {
                write!(f, "corrupt record on line {line}: {error}")
            }
            ReadError::TooLarge { limit } => write!(f, "file exceeds the {limit}-byte read limit"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Visitor-style reader: streams records so memory stays bounded by what
/// the consumer keeps.
pub struct Contents {
    pub header: Header,
    pub end: Option<End>,
    pub events: Vec<Event>,
    pub warnings: Vec<String>,
}

pub const DEFAULT_READ_LIMIT: u64 = 1 << 30;

pub fn read(
    path: &Path,
    limit: u64,
    on_sample: impl FnMut(&Sample),
) -> Result<Contents, ReadError> {
    let f = File::open(path).map_err(ReadError::Io)?;
    if f.metadata().map_err(ReadError::Io)?.len() > limit {
        return Err(ReadError::TooLarge { limit });
    }
    read_from(BufReader::new(f.take(limit)), on_sample)
}

pub fn read_from(
    r: impl BufRead,
    mut on_sample: impl FnMut(&Sample),
) -> Result<Contents, ReadError> {
    let mut lines = r.lines().enumerate().peekable();
    let header = match lines.next() {
        Some((_, Ok(l))) => {
            let v: serde_json::Value =
                serde_json::from_str(&l).map_err(|e| ReadError::NotARecording(e.to_string()))?;
            if v.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
                return Err(ReadError::NotARecording(
                    "missing nysm-recording header".into(),
                ));
            }
            let found = v
                .get("format_version")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            if found > FORMAT_VERSION || found == 0 {
                return Err(ReadError::UnsupportedVersion {
                    found,
                    supported: FORMAT_VERSION,
                });
            }
            match serde_json::from_value::<Record>(v) {
                Ok(Record::Header(h)) => *h,
                Ok(_) => {
                    return Err(ReadError::NotARecording(
                        "first record is not a header".into(),
                    ));
                }
                Err(e) => {
                    return Err(ReadError::Corrupt {
                        line: 1,
                        error: e.to_string(),
                    });
                }
            }
        }
        Some((_, Err(e))) => return Err(ReadError::Io(e)),
        None => return Err(ReadError::NotARecording("empty file".into())),
    };
    let mut c = Contents {
        header,
        end: None,
        events: Vec::new(),
        warnings: Vec::new(),
    };
    while let Some((i, line)) = lines.next() {
        let line = line.map_err(ReadError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let last = lines.peek().is_none();
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Sample(s)) => on_sample(&s),
            Ok(Record::Event(e)) => c.events.push(e),
            Ok(Record::End(e)) => c.end = Some(e),
            Ok(Record::Header(_)) => {
                return Err(ReadError::Corrupt {
                    line: i + 1,
                    error: "second header".into(),
                });
            }
            // A partial final line means the writer was killed mid-write.
            Err(_) if last => c
                .warnings
                .push(format!("ignored incomplete final line {}", i + 1)),
            Err(e) => {
                return Err(ReadError::Corrupt {
                    line: i + 1,
                    error: e.to_string(),
                });
            }
        }
    }
    if c.end.is_none() {
        c.warnings.push(
            "recording has no end record (interrupted?); results cover the data present".into(),
        );
    }
    Ok(c)
}
