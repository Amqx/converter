pub mod matching;

fn main() {
    let mut matcher = matching::Matcher::new();
    matcher.register_source(Box::new(matching::tachiyomi::Keiyoushi::new().unwrap()));
    matcher.register_source(Box::new(matching::aidoku::AmqxGit::new().unwrap()));
}
