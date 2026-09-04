use std::path::PathBuf;
use std::time::Duration;
use librqbit::{AddTorrent, AddTorrentOptions, Session, SessionOptions};
use torrentty::indexer::{build_client, search_nyaa};
use torrentty::util::DEFAULT_HTTPS_TRACKERS;

#[tokio::test]
async fn test_nyaa_search_and_metadata_fetch() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let client = build_client().expect("build client");
    println!("Searching Nyaa for 'SubsPlease Slime 20 1080p'...");
    let results = search_nyaa(&client, "SubsPlease Slime 20 1080p", 10).await.expect("search nyaa");
    assert!(!results.is_empty(), "expected search results from nyaa");
    println!("Found {} results from Nyaa!", results.len());

    let top = &results[0];
    println!("Top result: '{}' (seeders: {})", top.name, top.seeders);
    assert!(top.torrent_url.is_some(), "expected torrent_url to be extracted");
    let url = top.torrent_url.as_ref().unwrap();
    println!("Torrent URL: {url}");
    assert!(url.starts_with("https://nyaa.si/download/"));

    // Verify downloading .torrent bytes
    println!("Fetching .torrent file bytes...");
    let resp = client.get(url).send().await.expect("fetch torrent");
    assert!(resp.status().is_success());
    let bytes = resp.bytes().await.expect("bytes");
    assert!(bytes.len() > 1000, "torrent file should be valid bencoded data");
    println!("Successfully verified .torrent fetch ({} bytes)!", bytes.len());

    // Test adding to librqbit session with DEFAULT_HTTPS_TRACKERS
    let download_dir = PathBuf::from("/tmp/torrentty_nyaa_verify_test");
    let _ = std::fs::create_dir_all(&download_dir);

    let default_trackers: std::collections::HashSet<url::Url> = DEFAULT_HTTPS_TRACKERS
        .iter()
        .filter_map(|s| url::Url::parse(s).ok())
        .collect();

    let opts = SessionOptions {
        disable_dht_persistence: true,
        listen_port_range: Some(42400..42500),
        enable_upnp_port_forwarding: false,
        trackers: default_trackers,
        socks_proxy_url: None,
        ..Default::default()
    };

    let session = Session::new_with_opts(download_dir, opts).await.expect("session");
    let add_opts = AddTorrentOptions {
        overwrite: true,
        trackers: Some(DEFAULT_HTTPS_TRACKERS.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };

    let start = std::time::Instant::now();
    let resp = session.add_torrent(AddTorrent::TorrentFileBytes(bytes), Some(add_opts)).await.expect("add torrent");
    let elapsed = start.elapsed();
    println!("Torrent added to rqbit in {elapsed:?}!");
    assert!(elapsed < Duration::from_secs(2), "torrent addition should not deadlock");

    let handle = resp.into_handle().expect("handle");
    let stats = handle.stats();
    println!("Initial stats: total_bytes={}", stats.total_bytes);
    assert!(stats.total_bytes > 0, "metadata should be populated immediately from .torrent bytes");

    println!("Waiting to observe incoming peers and download progress...");
    for i in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stats = handle.stats();
        let live = stats.live.as_ref();
        let peer_count = live.map(|l| l.snapshot.peer_stats.live).unwrap_or(0);
        let fetched_bytes = live.map(|l| l.snapshot.fetched_bytes).unwrap_or(0);
        println!(
            "[{:>2}s] state={:?} peers={} fetched={} progress={}/{} (down_speed={})",
            i + 1,
            stats.state,
            peer_count,
            fetched_bytes,
            stats.progress_bytes,
            stats.total_bytes,
            stats.progress_bytes
        );
        if stats.progress_bytes > 0 || fetched_bytes > 0 {
            println!("SUCCESS: Downloaded bytes of Uma Musume Cinderella! fetched={}, progress={}", fetched_bytes, stats.progress_bytes);
            break;
        }
    }
}
