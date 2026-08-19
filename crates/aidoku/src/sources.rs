use converter_core::extensions::ExtResult;
use converter_core::extensions::types::{
    ExtensionMatch, ExtensionQuery, ExtensionRepository, ExtensionRepositoryInfo, RefreshFuture,
};

pub struct Amqx {}

impl Amqx {
    pub fn new() -> Self {
        todo!()
    }
}

impl ExtensionRepository for Amqx {
    fn info(&self) -> &ExtensionRepositoryInfo {
        todo!()
    }

    fn search(&self, query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
        todo!()
    }

    fn refresh(&mut self) -> RefreshFuture<'_> {
        todo!()
    }
}
