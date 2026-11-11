//! `cargo run -p voxa-proto --features update --example update_check -- owner/repo 0.0.1 .zip`
fn main() {
    let repo = std::env::args().nth(1).unwrap_or("dylan0760/voxa-client".into());
    let current = std::env::args().nth(2).unwrap_or(voxa_proto::APP_VERSION.into());
    let suffix = std::env::args().nth(3).unwrap_or(".msi".into());
    match voxa_proto::update::check(&repo, &current, |n| n.ends_with(&suffix)) {
        Ok(Some(r)) => println!("update {} -> {} ({}, {} bytes, sha256 {:?})", current, r.version, r.asset_name, r.size, r.sha256),
        Ok(None) => println!("up to date"),
        Err(e) => println!("error: {e}"),
    }
}
