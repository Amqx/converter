pub mod types;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum IRError {}

pub type IRResult<T> = Result<T, IRError>;
