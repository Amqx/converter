use crate::reader::AidokuReader;
use crate::sources::Amqx;
use crate::writer::AidokuWriter;

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
