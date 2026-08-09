use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadHistoryEntry {
    pub info_hash: String,
    pub name: String,
    pub target_path: PathBuf,
    pub added_at_epoch_secs: u64,
    pub completed_at_epoch_secs: Option<u64>,
}

#[must_use]
pub fn history_path() -> PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    PathBuf::from(home).join(".cache/torrentty/download-history.json")
}

pub fn load_history(path: &Path) -> Result<Vec<DownloadHistoryEntry>> {
    let Ok(file) = fs::File::open(path) else {
        return Ok(Vec::new());
    };
    let reader = std::io::BufReader::new(file);
    let entries: Vec<DownloadHistoryEntry> = serde_json::from_reader(reader)?;
    Ok(entries.into_iter().filter(|e| e.completed_at_epoch_secs.is_none() || e.target_path.exists()).collect())
}

pub fn upsert_history(path: &Path, entry: DownloadHistoryEntry) -> Result<()> {
    let mut entries = load_history(path)?;
    if let Some(existing) = entries.iter_mut().find(|e| e.info_hash == entry.info_hash) {
        *existing = entry;
    } else {
        entries.push(entry);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)?;
    let writer = std::io::BufWriter::new(file);
    serde_json::to_writer(writer, &entries)?;
    Ok(())
}

#[must_use]
pub fn now_epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
