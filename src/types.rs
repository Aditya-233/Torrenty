#[derive(Debug, Clone)]
pub struct Torrent {
    pub name: String,
    pub info_hash: String,
    pub magnet: Option<String>,
    pub torrent_url: Option<String>,
    pub seeders: u32,
    pub size_bytes: u64,
}

impl Torrent {
    #[must_use]
    pub fn resolved_magnet(&self) -> String {
        self.magnet.clone().unwrap_or_else(|| crate::util::build_magnet_link(&self.info_hash, &self.name))
    }
}
