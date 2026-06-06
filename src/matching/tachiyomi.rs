// Sources:
// 1: Keiyoushi's official repo (https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json)

use std::collections::{BTreeMap, HashMap};
use super::SourceId::{Tachiyomi};
use super::{Source, SourceData, SourceId};
use reqwest::blocking::Client;
use serde::{Deserialize};
use std::fmt;
use std::fmt::Formatter;

#[derive(Debug, Clone, Deserialize)]
struct TachiyomiEntry {
    name: String,
    urls: Vec<String>,
    ids: Vec<i64>,
}
impl fmt::Display for TachiyomiEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?}): {}", self.name, self.ids, self.urls.join(", "))
    }
}

#[derive(Deserialize, Debug)]
struct TachiyomiIndex {
    sources: Vec<TachiyomiIndexSource>
}

#[derive(Deserialize, Debug)]
struct TachiyomiIndexSource {
    name: String,
    id: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
}

pub struct Keiyoushi {
    entries: Vec<TachiyomiEntry>,

    by_name: HashMap<String, usize>,
    by_url: HashMap<String, usize>,
    by_id: HashMap<i64, usize>,
}

impl Keiyoushi {
    const ID: SourceId = Tachiyomi;
    const INDEX_URL: &str = "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";

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
            .user_agent("Tachiyomi-Source-Fetcher")
            .build()?;

        let sources: Vec<TachiyomiIndex> = client
            .get(Self::INDEX_URL)
            .send()?
            .error_for_status()?
            .json()?;

        let mut deduped: BTreeMap<(String, Vec<String>), Vec<i64>> = BTreeMap::new();
        for extension in sources {
            for source in extension.sources {
                let urls = vec![source.base_url];
                let id = source.id.parse::<i64>()?;

                deduped
                    .entry((source.name, urls))
                    .or_default()
                    .push(id);
            }
        }

        self.entries = deduped
            .into_iter()
            .map(|((name, urls), mut ids)| {
                ids.sort_unstable();
                ids.dedup();

                TachiyomiEntry {
                    name,
                    urls,
                    ids,
                }
            })
            .collect();

        for (idx, entry) in self.entries.iter().enumerate() {
            self.by_name.insert(entry.name.to_lowercase(), idx);

            for url in &entry.urls {
                self.by_url.insert(url.to_lowercase(), idx);
            }

            for id in &entry.ids {
                self.by_id.insert(*id, idx);
            }
        }

        Ok(())
    }

    fn print(&self) {
        for item in &self.entries {
            println!("{}", item);
        }
    }
}

impl Source for Keiyoushi {
    fn id(&self) -> SourceId {
        Self::ID
    }

    fn search(&self, query: &SourceData) -> super::Result<Vec<SourceData>> {
        todo!()
    }
}

mod test {
    use super::*;

    #[test]
    fn pull() {
        let keiyoushi = Keiyoushi::new();
        keiyoushi.unwrap().print();
    }
}
