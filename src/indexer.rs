use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

use crate::types::Torrent;
use crate::util::{parse_opt_string, parse_string, parse_u32, parse_u64};

pub fn build_client() -> Result<Client> {
    Client::builder().user_agent("torrentty/0.1").build().context("failed to build HTTP client")
}

pub async fn search_piratebay(client: &Client, query: &str, limit: usize) -> Result<Vec<Torrent>> {
    let response = client.get("https://apibay.org/q.php").query(&[("q", query)]).send().await?.error_for_status()?;

    let bytes = response.bytes().await?;
    let items: Vec<ApiTorrent> = serde_json::from_slice(&bytes)?;

    let results = items.into_iter().filter(|item| item.id != "0" && item.info_hash.as_deref().is_some_and(|hash| !hash.trim().is_empty())).map(Torrent::from).take(limit).collect();

    Ok(results)
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
        Self { name: value.name, info_hash: value.info_hash.unwrap_or_default(), magnet: value.magnet, seeders: value.seeders, size_bytes: value.size_bytes }
    }
}

pub async fn search_nyaa(client: &Client, query: &str, limit: usize) -> Result<Vec<Torrent>> {
    let response = client.get("https://nyaa.si/?page=rss").query(&[("q", query), ("c", "0_0"), ("f", "0")]).send().await?.error_for_status()?;

    let body = response.text().await?;
    let mut results = Vec::new();
    for item_str in body.split("<item>").skip(1) {
        let end = item_str.find("</item>").unwrap_or(item_str.len());
        let item_xml = &item_str[..end];

        let name = extract_xml_tag(item_xml, "<title>", "</title>").unwrap_or_default().to_string();
        let info_hash = extract_xml_tag(item_xml, "<nyaa:infoHash>", "</nyaa:infoHash>").unwrap_or_default().to_string();
        if info_hash.is_empty() {
            continue;
        }

        let size_str = extract_xml_tag(item_xml, "<nyaa:size>", "</nyaa:size>").unwrap_or_default();
        let size_bytes = parse_nyaa_size(size_str);
        let seeders = extract_xml_tag(item_xml, "<nyaa:seeders>", "</nyaa:seeders>").unwrap_or_default().parse().unwrap_or(0);
        let magnet = Some(crate::util::build_magnet_link(&info_hash, &name));

        results.push(Torrent { name, info_hash, magnet, seeders, size_bytes });

        if results.len() >= limit {
            break;
        }
    }
    Ok(results)
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
