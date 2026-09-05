use std::path::PathBuf;
use torrentty::storage::DownloadHistoryEntry;
use torrentty::types::Torrent;
use torrentty::util::{
    DEFAULT_HTTPS_TRACKERS, build_magnet_link, format_size, parse_opt_string, parse_string,
    parse_u32, parse_u64,
};

#[test]
fn test_format_size() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(500), "500 B");
    assert_eq!(format_size(1024), "1.00 KB");
    assert_eq!(format_size(1536), "1.50 KB");
    assert_eq!(format_size(1_048_576), "1.00 MB");
    assert_eq!(format_size(104_857_600), "100.00 MB");
    assert_eq!(format_size(1_073_741_824), "1.00 GB");
    assert_eq!(format_size(1_503_238_553), "1.40 GB");
}

#[test]
fn test_build_magnet_link() {
    let hash = "1a176246b6abe4f3912cf59e5d4cf7e8f4f70817";
    let name = "[SubsPlease] Slime S4 - 21 (1080p) [ABCD1234].mkv";
    let magnet = build_magnet_link(hash, name);

    assert!(magnet.starts_with("magnet:?xt=urn:btih:1a176246b6abe4f3912cf59e5d4cf7e8f4f70817"));
    assert!(magnet.contains("&dn="));
    // Check that spaces and brackets are URI-encoded
    assert!(!magnet.contains(' '));
    assert!(!magnet.contains('['));
    assert!(!magnet.contains(']'));
    // Check that trackers are included
    assert!(magnet.contains("&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"));
    assert!(magnet.contains("&tr=http%3A%2F%2Fnyaa.tracker.wf%3A7777%2Fannounce"));
}

#[test]
fn test_resolved_magnet_fallback() {
    let t_with_magnet = Torrent {
        name: "Test Torrent".to_string(),
        info_hash: "abcdef123456".to_string(),
        magnet: Some("magnet:?xt=urn:btih:abcdef123456&dn=custom".to_string()),
        torrent_url: None,
        seeders: 10,
        size_bytes: 1000,
    };
    assert_eq!(
        t_with_magnet.resolved_magnet(),
        "magnet:?xt=urn:btih:abcdef123456&dn=custom"
    );

    let t_without_magnet = Torrent {
        name: "Test Torrent".to_string(),
        info_hash: "abcdef123456".to_string(),
        magnet: None,
        torrent_url: None,
        seeders: 10,
        size_bytes: 1000,
    };
    let resolved = t_without_magnet.resolved_magnet();
    assert!(resolved.starts_with("magnet:?xt=urn:btih:abcdef123456"));
    assert!(resolved.contains("Test%20Torrent"));
}

#[test]
fn test_json_deserializers() {
    #[derive(serde::Deserialize)]
    struct TestItem {
        #[serde(deserialize_with = "parse_string")]
        s: String,
        #[serde(deserialize_with = "parse_opt_string")]
        opt_s: Option<String>,
        #[serde(deserialize_with = "parse_u64")]
        val64: u64,
        #[serde(deserialize_with = "parse_u32")]
        val32: u32,
    }

    let json_data = r#"{
        "s": "hello",
        "opt_s": 42,
        "val64": "1234567890123",
        "val32": "999"
    }"#;
    let item: TestItem = serde_json::from_str(json_data).expect("deserialize valid");
    assert_eq!(item.s, "hello");
    assert_eq!(item.opt_s, Some("42".to_string()));
    assert_eq!(item.val64, 1_234_567_890_123);
    assert_eq!(item.val32, 999);

    let json_null = r#"{
        "s": "foo",
        "opt_s": null,
        "val64": "100",
        "val32": "50"
    }"#;
    let item2: Result<TestItem, _> = serde_json::from_str(json_null);
    assert!(item2.is_ok());
    assert_eq!(item2.unwrap().opt_s, None);
}

#[test]
fn test_history_serialization() {
    let entry = DownloadHistoryEntry {
        info_hash: "deadbeef12345678".to_string(),
        name: "Demo Movie 2026".to_string(),
        target_path: PathBuf::from("/home/user/Downloads/Demo Movie 2026.mp4"),
        added_at_epoch_secs: 1_700_000_000,
        completed_at_epoch_secs: Some(1_700_000_050),
    };

    let serialized = serde_json::to_string(&entry).expect("serialize");
    let deserialized: DownloadHistoryEntry =
        serde_json::from_str(&serialized).expect("deserialize");
    assert_eq!(entry, deserialized);
}

#[test]
fn test_default_trackers_validity() {
    assert!(!DEFAULT_HTTPS_TRACKERS.is_empty());
    for tracker in DEFAULT_HTTPS_TRACKERS {
        assert!(tracker.starts_with("http://") || tracker.starts_with("https://"));
        assert!(tracker.ends_with("/announce"));
        assert!(url::Url::parse(tracker).is_ok());
    }
}
