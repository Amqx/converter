use clap::ValueEnum;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
pub enum ReaderId {
    Aidoku,
    Tachiyomi,
}

impl fmt::Display for ReaderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReaderId::Aidoku => write!(f, "Aidoku"),
            ReaderId::Tachiyomi => write!(f, "Tachiyomi"),
        }
    }
}
