//! Complete session logs live on disk. Only the visible page is decoded for UI.
use anyhow::{Context, Result};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};
use tempfile::NamedTempFile;

struct Store {
    file: NamedTempFile,
    starts: Vec<u64>,
    bytes: u64,
}

#[derive(Clone)]
pub struct History(Arc<Mutex<Store>>);

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub lines: usize,
    pub bytes: u64,
}

impl Store {
    fn lines(&self) -> usize {
        self.starts.len() - usize::from(self.starts.last() == Some(&self.bytes))
    }
}

impl History {
    pub fn new() -> Result<Self> {
        Ok(Self(Arc::new(Mutex::new(Store {
            file: tempfile::Builder::new()
                .prefix("efc-logs-")
                .tempfile()
                .context("Could not create the log history file")?,
            starts: vec![0],
            bytes: 0,
        }))))
    }

    pub fn append(&self, bytes: &[u8]) -> Result<()> {
        let mut store = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Log history lock failed"))?;
        store.file.seek(SeekFrom::End(0))?;
        store
            .file
            .write_all(bytes)
            .context("Could not write log history (check free disk space)")?;
        let base = store.bytes;
        for (i, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' {
                store.starts.push(base + i as u64 + 1);
            }
        }
        store.bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        self.0
            .lock()
            .map(|s| Stats {
                lines: s.lines(),
                bytes: s.bytes,
            })
            .unwrap_or_default()
    }

    pub fn page(&self, start: usize, count: usize) -> Result<Vec<String>> {
        let mut store = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Log history lock failed"))?;
        if start >= store.lines() || count == 0 {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(count).min(store.lines());
        let from = store.starts[start];
        let to = store.starts.get(end).copied().unwrap_or(store.bytes);
        store.file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut store.file)
            .take(to - from)
            .read_to_end(&mut bytes)?;
        drop(store);
        // Decode whole lines, so UTF-8 split across Docker read chunks survives.
        let text = String::from_utf8_lossy(&bytes);
        Ok(text
            .split_terminator('\n')
            .map(crate::ui::clean_text)
            .collect())
    }

    /// Find the next matching line, wrapping once. A worker calls this in pages,
    /// allowing the UI and log writer to continue during a large-history search.
    pub fn find(
        &self,
        query: &str,
        start: usize,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<usize>> {
        let count = self.stats().lines;
        let start = start.min(count);
        let needle = query.to_lowercase();
        for (from, to) in [(start, count), (0, start)] {
            let mut line = from;
            while line < to {
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(None);
                }
                for (i, text) in self.page(line, (to - line).min(128))?.iter().enumerate() {
                    if text.to_lowercase().contains(&needle) {
                        return Ok(Some(line + i));
                    }
                }
                line = line.saturating_add(128);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_history_exceeds_old_limits_and_keeps_split_unicode_and_partial_lines() {
        let history = History::new().unwrap();
        for i in 0..70_000 {
            history
                .append(format!("{i:06} {}\n", "complete history ".repeat(3)).as_bytes())
                .unwrap();
        }
        history.append(b"partial \xc3").unwrap();
        history.append(b"\xa9\x1b[31m red\x1b[0m").unwrap();
        assert_eq!(history.stats().lines, 70_001);
        assert!(history.stats().bytes > 2 * 1024 * 1024);
        assert!(history.page(0, 1).unwrap()[0].starts_with("000000"));
        assert!(history.page(69_999, 1).unwrap()[0].starts_with("069999"));
        assert_eq!(history.page(70_000, 1).unwrap(), ["partial é red"]);
        history.append(b"\n").unwrap();
        assert_eq!(history.stats().lines, 70_001);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(history.find("000000", 60_000, &cancel).unwrap(), Some(0));
    }
}
