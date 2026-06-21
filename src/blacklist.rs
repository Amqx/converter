use crate::reader::ReaderId;

pub struct BlacklistedSource {
    pub reader: ReaderId,
    pub id: &'static str,
    pub reason: &'static str,
}

const BLACKLIST: &[BlacklistedSource] = &[
    // Tachiyomi Blacklist
    BlacklistedSource { reader: ReaderId::Tachiyomi, id: "0",    reason: "local source" },
    BlacklistedSource { reader: ReaderId::Tachiyomi, id: "6969", reason: "merged source" },
    BlacklistedSource { reader: ReaderId::Tachiyomi, id: "6902", reason: "Ex-Hentai source" },
    
    // Aidoku Blacklist
    BlacklistedSource { reader: ReaderId::Aidoku, id: "local", reason: "local source"},
];

/// Returns the blacklist reason if `source_id` is blacklisted for `reader`.
pub fn reason(reader: ReaderId, source_id: &str) -> Option<&'static str> {
    BLACKLIST
        .iter()
        .find(|entry| entry.reader == reader && entry.id == source_id)
        .map(|entry| entry.reason)
}
