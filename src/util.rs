use serde::Deserialize;
use std::fmt::Write;

#[must_use]
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_073_741_824.0 {
        format!("{:.2} GB", b / 1_073_741_824.0)
    } else if b >= 1_048_576.0 {
        format!("{:.2} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.2} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub const DEFAULT_HTTPS_TRACKERS: &[&str] = &[
    "https://tracker.pmman.tech:443/announce",
    "https://tracker.tamersunion.org:443/announce",
    "https://tracker.nekobt.to/api/tracker/public/announce",
    "http://nyaa.tracker.wf:7777/announce",
    "https://tracker.nekomi.cn:443/announce",
    "https://1337.abcvg.info:443/announce",
    "https://tracker.leechshield.link:443/announce",
    "https://tracker.7471.top:443/announce",
    "https://tracker.foreverpirates.co:443/announce",
];

const PRE_ENCODED_TRACKERS: &[&str] = &[
    // Universal high-speed public UDP trackers (ngosang trackerslist top tier)
    "udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce",
    "udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce",
    "udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce",
    "udp%3A%2F%2Fopen.demonii.com%3A1337%2Fannounce",
    "udp%3A%2F%2Ftracker.therarbg.to%3A6969%2Fannounce",
    "udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce",
    "udp%3A%2F%2Fexplodie.org%3A6969%2Fannounce",
    "udp%3A%2F%2Fzer0day.ch%3A1337%2Fannounce",
    "udp%3A%2F%2Ftracker.qu.ax%3A6969%2Fannounce",
    // Universal HTTPS trackers
    "https%3A%2F%2Ftracker.pmman.tech%3A443%2Fannounce",
    "https%3A%2F%2Ftracker.tamersunion.org%3A443%2Fannounce",
    "https%3A%2F%2Ftracker.nekobt.to%2Fapi%2Ftracker%2Fpublic%2Fannounce",
    // Dedicated anime / Nyaa trackers
    "http%3A%2F%2Fnyaa.tracker.wf%3A7777%2Fannounce",
    "https%3A%2F%2Ftracker.nekomi.cn%3A443%2Fannounce",
    "https%3A%2F%2F1337.abcvg.info%3A443%2Fannounce",
    "https%3A%2F%2Ftracker.leechshield.link%3A443%2Fannounce",
    "https%3A%2F%2Ftracker.7471.top%3A443%2Fannounce",
    "https%3A%2F%2Ftracker.foreverpirates.co%3A443%2Fannounce",
];

#[must_use]
pub fn build_magnet_link(info_hash: &str, name: &str) -> String {
    let mut magnet = format!("magnet:?xt=urn:btih:{info_hash}&dn=");
    for b in name.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => magnet.push(b as char),
            _ => {
                let _ = write!(magnet, "%{b:02X}");
            }
        }
    }
    for t in PRE_ENCODED_TRACKERS {
        let _ = write!(magnet, "&tr={t}");
    }
    magnet
}

pub fn parse_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    parse_opt_string(d).and_then(|opt| opt.ok_or_else(|| serde::de::Error::custom("expected string")))
}

pub fn parse_opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

pub fn parse_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let s = parse_string(d)?;
    s.trim().parse().map_err(serde::de::Error::custom)
}

pub fn parse_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    parse_u64(d).and_then(|n| u32::try_from(n).map_err(serde::de::Error::custom))
}

use std::fs::OpenOptions;
use std::io::Write as IoWrite;
use std::sync::Mutex;

static LOG_MUTEX: Mutex<()> = Mutex::new(());

pub fn log(level: &str, msg: &str) {
    let _guard = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(home).join(".cache/torrentty");
    let _ = std::fs::create_dir_all(&dir);
    let log_file = dir.join("torrentty.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_file) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = now.as_secs();
        let millis = now.subsec_millis();
        let _ = writeln!(f, "[{secs}.{millis:03}] [{level}] {msg}");
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::util::log("INFO", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::util::log("WARN", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::util::log("ERROR", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::util::log("DEBUG", &format!($($arg)*))
    };
}

