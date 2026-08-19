pub mod reader;
pub mod writer;

use thiserror::Error;
use crate::extensions::types::Extension;

#[derive(Debug, PartialEq, Clone)]
pub struct SkippedExtension {
    pub reason: String,
    pub extension: Extension,
}

#[derive(Debug, PartialEq, Clone)]
pub struct FailedExtension {
    pub reason: String,
    pub extension: Extension,
}

#[derive(Error, Debug)]
pub enum ReadError {}

#[derive(Error, Debug)]
pub enum WriteError {}
