use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

use crate::types::Torrent;
use crate::util::{parse_opt_string, parse_string, parse_u32, parse_u64};

fn is_local_port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(60),
    )
    .is_ok()
}

pub fn build_client() -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(10));

    let mut proxy_str = None;

    // Check SOCKS proxy environment variables first (remote DNS via socks5h)
    for var in ["ALL_PROXY", "all_proxy", "SOCKS_PROXY", "socks_proxy"] {
        if let Ok(val) = std::env::var(var) {
            let val = val.trim();
            if !val.is_empty() {
                let formatted = if let Some(stripped) = val.strip_prefix("socks5://") {
                    format!("socks5h://{stripped}")
                } else {
                    val.to_string()
                };
                crate::log_info!("Found SOCKS proxy env {}={}", var, formatted);
                proxy_str = Some(formatted);
                break;
            }
        }
    }

    // Check HTTP proxy environment variables
    if proxy_str.is_none() {
        for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
            if let Ok(val) = std::env::var(var) {
                let val = val.trim();
                if !val.is_empty() {
                    crate::log_info!("Found HTTP proxy env {}={}", var, val);
                    proxy_str = Some(val.to_string());
                    break;
                }
            }
        }
    }

    // Fallback: auto-detect active local DPI bypass ports (1080 SOCKS5 preferred, 8080 HTTP fallback)
    if proxy_str.is_none() {
        if is_local_port_open(1080) {
            crate::log_info!("Auto-detected active DPI bypass SOCKS5 engine on 127.0.0.1:1080");
            proxy_str = Some("socks5h://127.0.0.1:1080".to_string());
        } else if is_local_port_open(8080) {
            crate::log_info!("Auto-detected active HTTP proxy on 127.0.0.1:8080");
            proxy_str = Some("http://127.0.0.1:8080".to_string());
        }
    }

    if let Some(ref p) = proxy_str {
        match reqwest::Proxy::all(p) {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
                crate::log_info!("Configured reqwest HTTP client with proxy: {}", p);
            }
            Err(e) => {
                crate::log_warn!("Failed to configure proxy '{}': {:#}", p, e);
            }
        }
    } else {
        crate::log_info!("No proxy configured for reqwest HTTP client (direct connection)");
    }

    builder.build().context("failed to build HTTP client")
}

pub async fn search_piratebay(client: &Client, query: &str, limit: usize) -> Result<Vec<Torrent>> {
    crate::log_info!("Starting PirateBay search for query='{}', limit={}", query, limit);
    let start = std::time::Instant::now();
    let req = client.get("https://apibay.org/q.php").query(&[("q", query)]);

    let send_res = tokio::time::timeout(std::time::Duration::from_secs(4), req.send()).await;
    match send_res {
        Ok(Ok(resp)) => {
            let status = resp.status();
            crate::log_info!("PirateBay HTTP response status={} (elapsed: {:?})", status, start.elapsed());
            if !status.is_success() {
                crate::log_warn!("PirateBay non-success response: {}", status);
                return Ok(Vec::new());
            }
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    crate::log_warn!("Failed to read PirateBay response body: {:#}", e);
                    return Ok(Vec::new());
                }
            };
            crate::log_info!("PirateBay received {} bytes in {:?}", bytes.len(), start.elapsed());
            let items: Vec<ApiTorrent> = match serde_json::from_slice(&bytes) {
                Ok(it) => it,
                Err(e) => {
                    crate::log_warn!("Failed to parse PirateBay JSON: {:#}", e);
                    return Ok(Vec::new());
                }
            };

            let results: Vec<Torrent> = items
                .into_iter()
                .filter(|item| item.id != "0" && item.info_hash.as_deref().is_some_and(|hash| !hash.trim().is_empty()))
                .map(Torrent::from)
                .take(limit)
                .collect();

            crate::log_info!("PirateBay parsed {} torrents in {:?}", results.len(), start.elapsed());
            Ok(results)
        }
        Ok(Err(e)) => {
            crate::log_warn!("PirateBay request failed: {:#} (elapsed: {:?})", e, start.elapsed());
            Ok(Vec::new())
        }
        Err(_) => {
            crate::log_warn!("PirateBay request timed out after 4s (elapsed: {:?})", start.elapsed());
            Ok(Vec::new())
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiTorrent {
    #[serde(deserialize_with = "parse_string")]
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "parse_opt_string")]
    pub info_hash: Option<String>,
    #[serde(default, deserialize_with = "parse_opt_string")]
    pub magnet: Option<String>,
    #[serde(deserialize_with = "parse_u32")]
    pub seeders: u32,
    #[serde(rename = "size", deserialize_with = "parse_u64")]
    pub size_bytes: u64,
}

impl From<ApiTorrent> for Torrent {
    fn from(value: ApiTorrent) -> Self {
        Self {
            name: value.name,
            info_hash: value.info_hash.unwrap_or_default(),
            magnet: value.magnet,
            torrent_url: None,
            seeders: value.seeders,
            size_bytes: value.size_bytes,
        }
    }
}

pub async fn search_nyaa(client: &Client, query: &str, limit: usize) -> Result<Vec<Torrent>> {
    crate::log_info!("Starting Nyaa.si search for query='{}', limit={}", query, limit);
    let start = std::time::Instant::now();
    let mut last_err = None;

    for attempt in 1..=3 {
        crate::log_info!("Nyaa attempt {}/3 for query='{}'", attempt, query);
        let req = client
            .get("https://nyaa.si/?page=rss")
            .query(&[("q", query), ("c", "0_0"), ("f", "0")]);

        let send_res = tokio::time::timeout(std::time::Duration::from_secs(6), req.send()).await;
        match send_res {
            Ok(Ok(resp)) => {
                let status = resp.status();
                crate::log_info!("Nyaa attempt {} HTTP status={} (elapsed: {:?})", attempt, status, start.elapsed());
                match resp.error_for_status() {
                    Ok(response) => {
                        let body = match response.text().await {
                            Ok(b) => b,
                            Err(e) => {
                                crate::log_warn!("Failed to read Nyaa response body: {:#}", e);
                                last_err = Some(anyhow::anyhow!(e));
                                continue;
                            }
                        };
                        crate::log_info!("Nyaa body received: {} bytes in {:?}", body.len(), start.elapsed());
                        let mut results = Vec::new();
                        for item_str in body.split("<item>").skip(1) {
                            let end = item_str.find("</item>").unwrap_or(item_str.len());
                            let item_xml = &item_str[..end];

                            let name = extract_xml_tag(item_xml, "<title>", "</title>").unwrap_or_default().to_string();
                            let info_hash = extract_xml_tag(item_xml, "<nyaa:infoHash>", "</nyaa:infoHash>").unwrap_or_default().to_string();
                            if info_hash.is_empty() {
                                continue;
                            }

                            let torrent_url = extract_xml_tag(item_xml, "<link>", "</link>").map(|s| s.to_string());
                            let size_str = extract_xml_tag(item_xml, "<nyaa:size>", "</nyaa:size>").unwrap_or_default();
                            let size_bytes = parse_nyaa_size(size_str);
                            let seeders = extract_xml_tag(item_xml, "<nyaa:seeders>", "</nyaa:seeders>").unwrap_or_default().parse().unwrap_or(0);
                            let magnet = Some(crate::util::build_magnet_link(&info_hash, &name));

                            results.push(Torrent {
                                name,
                                info_hash,
                                magnet,
                                torrent_url,
                                seeders,
                                size_bytes,
                            });

                            if results.len() >= limit {
                                break;
                            }
                        }
                        crate::log_info!("Nyaa parsed {} torrents in {:?}", results.len(), start.elapsed());
                        return Ok(results);
                    }
                    Err(e) => {
                        crate::log_warn!("Nyaa HTTP error status: {:#}", e);
                        last_err = Some(anyhow::anyhow!(e));
                    }
                }
            }
            Ok(Err(e)) => {
                crate::log_warn!("Nyaa connection error: {:#} (elapsed: {:?})", e, start.elapsed());
                last_err = Some(anyhow::anyhow!(e));
            }
            Err(_) => {
                crate::log_warn!("Nyaa request timed out after 6s (elapsed: {:?})", start.elapsed());
                last_err = Some(anyhow::anyhow!("request timed out after 6s"));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    let err = last_err.unwrap_or_else(|| anyhow::anyhow!("failed to search nyaa after 3 attempts"));
    crate::log_error!("Nyaa search totally failed: {:#}", err);
    Err(err)
}

fn extract_xml_tag<'a>(xml: &'a str, start_tag: &str, end_tag: &str) -> Option<&'a str> {
    let start = xml.find(start_tag)? + start_tag.len();
    let end = xml[start..].find(end_tag)?;
    let content = &xml[start..start + end];
    if content.starts_with("<![CDATA[") && content.ends_with("]]>") { Some(&content[9..content.len() - 3]) } else { Some(content) }
}

fn parse_nyaa_size(size_str: &str) -> u64 {
    let s = size_str.trim().to_lowercase();
    let val: f64 = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().parse().unwrap_or(0.0);
    let mult = if s.contains("tib") || s.contains("tb") {
        1024.0 * 1024.0 * 1024.0 * 1024.0
    } else if s.contains("gib") || s.contains("gb") {
        1024.0 * 1024.0 * 1024.0
    } else if s.contains("mib") || s.contains("mb") {
        1024.0 * 1024.0
    } else if s.contains("kib") || s.contains("kb") {
        1024.0
    } else {
        1.0
    };
    (val * mult).max(0.0) as u64
}
