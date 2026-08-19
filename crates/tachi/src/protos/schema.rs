//! Regenerates [`super::tachiyomisy`] from `TachiyomiSY`'s Kotlin sources.

use std::fmt::Write as _;
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

const REPO: &str = "jobobby04/TachiyomiSY";
const MODELS: &str = "app/src/main/java/eu/kanade/tachiyomi/data/backup/models";
const USER_AGENT: &str = concat!("converter-tachi/", env!("CARGO_PKG_VERSION"));
const PREAMBLE: &str = r#"syntax = "proto2";

enum UpdateStrategy {
  ALWAYS_UPDATE = 0;
  ONLY_FETCH_ONCE = 1;
}

message PreferenceValue {
  required string type = 1;
  required bytes truevalue = 2;
}
"#;

const CLASS: &str =
    r"(?m)^(?:data )?class (?P<name>\w+)\((?P<defs>(?:[^)(]+|\((?:[^)(]+|\([^)(]*\))*\))*)\)";

const FIELD: &str = r"(?m)(?:^\s*@ProtoNumber\((?P<number>\d+)\)\s*|data class \w+\(|^)va[rl]\s+(?P<name>\w+):\s+(?:(?:List|Set)<(?P<list>\w+)>|(?P<type>\w+))(?P<optional>\?|\s+=)?";

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("could not reach GitHub: {0}")]
    Fetch(#[from] reqwest::Error),
    #[error("GitHub's listing of {MODELS} did not parse: {0}")]
    Listing(#[from] serde_json::Error),
    #[error("bad pattern: {0}")]
    Pattern(#[from] regex::Error),
    #[error("could not write the schema: {0}")]
    Io(#[from] std::io::Error),
    #[error("prost rejected the generated schema: {0}")]
    Codegen(String),
}

/// A Kotlin type that protobuf has a scalar for. Anything else is the name of another model, and
/// passes through untouched so it lands as a message or enum reference.
fn scalar(kotlin: &str) -> Option<&'static str> {
    Some(match kotlin {
        "String" | "Char" => "string",
        "Int" => "int32",
        "Long" => "int64",
        "Boolean" => "bool",
        "Float" => "float",
        "ByteArray" => "bytes",
        _ => return None,
    })
}

/// Turns every data class in one Kotlin file into a protobuf message.
pub fn parse_model(source: &str) -> Result<String, SchemaError> {
    let class = Regex::new(CLASS)?;
    let field = Regex::new(FIELD)?;
    let mut out = String::new();

    for model in class.captures_iter(source) {
        let (Some(name), Some(defs)) = (model.name("name"), model.name("defs")) else {
            continue;
        };
        let _ = writeln!(out, "message {} {{", name.as_str());

        for param in field.captures_iter(defs.as_str()) {
            let Some(ident) = param.name("name") else {
                continue;
            };
            let list = param.name("list").map(|m| m.as_str());
            let kotlin = param.name("type").map(|m| m.as_str());

            let label = if list.is_some() || kotlin == Some("ByteArray") {
                "repeated"
            } else if param.name("optional").is_some() {
                // A Kotlin default or a `?` is the only hint that the writer may omit the field.
                "optional"
            } else {
                "required"
            };

            let declared = kotlin.or(list).unwrap_or_default();
            let ty = scalar(declared).unwrap_or(declared);
            // An unannotated field is the sole member of its class, so 1 is the only number free.
            let number = param.name("number").map_or("1", |m| m.as_str());

            let _ = writeln!(out, "  {label} {ty} {} = {number};", ident.as_str());
        }

        out.push_str("}\n\n");
    }

    Ok(out)
}

/// One page of GitHub's contents API.
async fn list(client: &reqwest::Client, url: &str) -> Result<Vec<Entry>, SchemaError> {
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&body)?)
}

/// Every `.kt` under the models directory, one subdirectory deep.
async fn fetch_models() -> Result<Vec<(String, String)>, SchemaError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;

    let root = format!("https://api.github.com/repos/{REPO}/contents/{MODELS}");
    let mut files = Vec::new();

    for entry in list(&client, &root).await? {
        match entry.kind.as_str() {
            "file" => files.push(entry),
            "dir" => files.extend(
                list(&client, &entry.url)
                    .await?
                    .into_iter()
                    .filter(|sub| sub.kind == "file"),
            ),
            _ => {}
        }
    }

    let mut models = Vec::new();
    for file in files {
        let Some(url) = file.download_url else {
            continue;
        };
        let source = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        models.push((file.name, source));
    }

    Ok(models)
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
    download_url: Option<String>,
}

/// Assembles the whole schema, keeping each model's filename as a comment so a diff points at
/// the Kotlin file that moved.
pub fn render(models: &[(String, String)]) -> Result<String, SchemaError> {
    let mut schema = String::from(PREAMBLE);
    for (name, source) in models {
        let _ = writeln!(schema, "\n// {name}");
        schema.push_str(&parse_model(source)?);
    }
    Ok(schema)
}

/// Writes `tachiyomisy.proto` next to `dir` and hands it to prost.
pub fn compile(dir: &Path, schema: &str) -> Result<(), SchemaError> {
    let proto = dir.join("tachiyomisy.proto");
    std::fs::write(&proto, schema)?;

    let mut config = prost_build::Config::new();
    config
        .out_dir(dir)
        // The schema declares no package, so without this prost writes `_.rs`.
        .default_package_filename("tachiyomisy");

    config
        .compile_protos(&[&proto], &[dir])
        .map_err(|e| SchemaError::Codegen(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_come_from_the_annotation() {
        let kt = r#"
data class BackupSource(
    @ProtoNumber(1) var name: String = "",
    @ProtoNumber(2) var sourceId: Long,
)
"#;
        assert_eq!(
            parse_model(kt).unwrap(),
            "message BackupSource {\n  optional string name = 1;\n  required int64 sourceId = 2;\n}\n\n"
        );
    }

    #[test]
    fn an_annotation_may_sit_on_its_own_line() {
        // SY splits the annotation off whenever a `@Deprecated` needs its own line; reading the
        // field as unannotated would silently renumber it to 1.
        let kt = r#"
data class BackupTracking(
    @Deprecated("Use mediaId instead", level = DeprecationLevel.WARNING)
    @ProtoNumber(3)
    var mediaIdInt: Int = 0,
)
"#;
        assert!(
            parse_model(kt)
                .unwrap()
                .contains("optional int32 mediaIdInt = 3;")
        );
    }

    #[test]
    fn commented_out_fields_are_dropped() {
        // These are numbers the fork has retired. Emitting them would claim a backup carries a
        // field that no writer has produced in years.
        let kt = r"
data class BackupCategory(
    @ProtoNumber(1) var name: String,
    // @ProtoNumber(3) val updateInterval: Int = 0, 1.x value not used in 0.x
    /*@ProtoNumber(600) var mangaOrder: List<Long> = emptyList(),*/
    @ProtoNumber(601) var version: Long = 0,
)
";
        let proto = parse_model(kt).unwrap();
        assert!(!proto.contains("updateInterval"));
        assert!(!proto.contains("mangaOrder"));
        assert!(proto.contains("optional int64 version = 601;"));
    }

    #[test]
    fn collections_and_byte_arrays_repeat() {
        let kt = r"
data class BackupManga(
    @ProtoNumber(7) var genre: List<String> = emptyList(),
    @ProtoNumber(16) var chapters: List<BackupChapter> = emptyList(),
    @ProtoNumber(112) var memo: ByteArray = JsonObjectEmptyBytes,
    @ProtoNumber(805) var customGenre: List<String>? = null,
)
";
        let proto = parse_model(kt).unwrap();
        assert!(proto.contains("repeated string genre = 7;"));
        assert!(proto.contains("repeated BackupChapter chapters = 16;"));
        assert!(proto.contains("repeated bytes memo = 112;"));
        assert!(proto.contains("repeated string customGenre = 805;"));
    }

    #[test]
    fn unknown_types_pass_through_as_message_references() {
        let kt = r"
data class BackupManga(
    @ProtoNumber(105) var updateStrategy: UpdateStrategy = UpdateStrategy.ALWAYS_UPDATE,
    @ProtoNumber(601) var flatMetadata: BackupFlatMetadata? = null,
)
";
        let proto = parse_model(kt).unwrap();
        assert!(proto.contains("optional UpdateStrategy updateStrategy = 105;"));
        assert!(proto.contains("optional BackupFlatMetadata flatMetadata = 601;"));
    }

    #[test]
    fn a_lone_field_on_the_class_line_gets_number_one() {
        // The `PreferenceValue` subtypes are written this way and carry no annotation.
        let kt = r"data class StringSetPreferenceValue(val value: Set<String>) : PreferenceValue()";
        assert_eq!(
            parse_model(kt).unwrap(),
            "message StringSetPreferenceValue {\n  repeated string value = 1;\n}\n\n"
        );
    }

    /// Rewrites the checked-in schema and bindings from SY's current `master`. Ignored because it
    /// hits the network and edits the tree.
    #[ignore = "regenerates checked-in sources from GitHub"]
    #[tokio::test]
    async fn regenerate() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/protos");
        let models = fetch_models().await.unwrap();
        let schema = render(&models).unwrap();
        compile(&dir, &schema).unwrap();
    }
}
