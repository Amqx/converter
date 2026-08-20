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

use crate::reader::AidokuReader;
use crate::sources::Amqx;
use crate::writer::AidokuWriter;

pub mod quirks;
mod reader;
mod sources;
mod writer;

converter_core::declare_format! {
    pub Aidoku = "aidoku" {
        reader: AidokuReader::new(),
        writer: AidokuWriter::new(),
        repositories: [Amqx::new()]
    }
}
