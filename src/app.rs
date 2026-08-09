use crate::indexer::build_client;
use crate::tui::run_search_tui;
use anyhow::{Result, anyhow};
use librqbit::Session;
use std::path::PathBuf;

/// Runs the application, initializing session and running the TUI.
pub async fn run() -> Result<()> {
    let download_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let opts = librqbit::SessionOptions { listen_port_range: Some(42400..42500), enable_upnp_port_forwarding: true, ..Default::default() };

    let session = Session::new_with_opts(download_dir, opts).await.map_err(|e| anyhow!("failed to initialize rqbit session: {e}"))?;

    let history_path = crate::storage::history_path();
    let history_entries = crate::storage::load_history(&history_path)?;

    let client = build_client()?;

    run_search_tui(session, history_entries, history_path, client).await
}
