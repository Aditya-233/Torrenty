#[derive(Debug, Clone)]
pub(crate) struct Torrent {
    pub(crate) name: String,
    pub(crate) info_hash: String,
    pub(crate) magnet: Option<String>,
    pub(crate) seeders: u32,
    pub(crate) size_bytes: u64,
}

impl Torrent {
    #[must_use]
    pub(crate) fn resolved_magnet(&self) -> String {
        self.magnet.clone().unwrap_or_else(|| crate::util::build_magnet_link(&self.info_hash, &self.name))
    }
}
