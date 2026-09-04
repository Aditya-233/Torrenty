use crate::indexer::build_client;
use crate::tui::run_search_tui;
use anyhow::{anyhow, Result};
use librqbit::Session;
use std::collections::HashSet;

/// Runs the application, initializing session and running the TUI.
pub async fn run() -> Result<()> {
    let download_dir = crate::storage::default_download_dir();

    let default_trackers: HashSet<url::Url> = crate::util::DEFAULT_HTTPS_TRACKERS
        .iter()
        .filter_map(|s| url::Url::parse(s).ok())
        .collect();

    let opts = librqbit::SessionOptions {
        disable_dht_persistence: true,
        listen_port_range: Some(42400..42500),
        enable_upnp_port_forwarding: false,
        trackers: default_trackers,
        socks_proxy_url: None,
        ..Default::default()
    };

    let session = Session::new_with_opts(download_dir, opts)
        .await
        .map_err(|e| anyhow!("failed to initialize rqbit session: {e}"))?;

    let history_path = crate::storage::history_path();
    let history_entries = crate::storage::load_history(&history_path)?;

    let client = build_client()?;

    run_search_tui(session, history_entries, history_path, client).await
}


