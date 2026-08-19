#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::float_cmp
    )
)]

use crate::keiyoushi::Keiyoushi;
use crate::reader::TachiReader;

mod keiyoushi;
mod protos;
mod reader;
mod writer;

converter_core::declare_format! {
    pub Tachiyomi = "tachiyomi" {
        reader: TachiReader::new(),
        repositories: [Keiyoushi::new()],
    }
}
