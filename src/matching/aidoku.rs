// Sources:
// 1: Amqx source index (https://amqx.github.io/sources/index.min.json)

use super::SourceId::Aidoku;
use super::{Source, SourceData, SourceId};
use std::collections::HashMap;
use reqwest::blocking::Client;
use serde::{Deserialize};
use std::fmt;
use std::fmt::Formatter;

#[derive(Debug, Clone, Deserialize)]
struct AidokuEntry {
    name: String,
    id: String,
    urls: Vec<String>,
}
impl fmt::Display for AidokuEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.name, self.id, self.urls.join(", "))
    }
}

#[derive(Deserialize, Debug)]
struct AidokuIndex {
    sources: Vec<AidokuIndexSource>,
}

#[derive(Deserialize, Debug)]
struct AidokuIndexSource {
    id: String,
    name: String,

    #[serde(rename = "baseURL")]
    base_url: String,
}

pub struct AmqxGit {
    entries: Vec<AidokuEntry>,

    by_name: HashMap<String, usize>,
    by_url: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
}

impl AmqxGit {
    const ID: SourceId = Aidoku;
    const INDEX_URL: &str = "https://amqx.github.io/sources/index.min.json";

    pub fn new() -> super::Result<Self> {
        let mut s = Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_url: HashMap::new(),
            by_id: HashMap::new(),
        };
        s.get()?;
        Ok(s)
    }

    fn get(&mut self) -> super::Result<()> {
        let client = Client::builder()
            .user_agent("Aidoku-Source-Fetcher")
            .build()?;

        let index: AidokuIndex = client
            .get(Self::INDEX_URL)
            .send()?
            .error_for_status()?
            .json()?;

        self.entries = index
            .sources
            .into_iter()
            .map(|source| AidokuEntry {
                name: source.name,
                id: source.id,
                urls: vec![source.base_url],
            })
            .collect();

        for (idx, entry) in self.entries.iter().enumerate() {
            self.by_name.insert(entry.name.to_lowercase(), idx);
            self.by_id.insert(entry.id.to_lowercase(), idx);

            for url in &entry.urls {
                self.by_url.insert(url.to_lowercase(), idx);
            }
        }

        Ok(())
    }

    fn print(&self) {
        for item in &self.entries {
            println!("{}", item)
        }
    }
}

impl Source for AmqxGit {
    fn id(&self) -> SourceId {
        Self::ID
    }

    fn search(&self, query: &SourceData) -> super::Result<Vec<SourceData>> {
        todo!()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn pull() {
        let aidoku = AmqxGit::new();
        aidoku.unwrap().print()
    }
}
