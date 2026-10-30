//! Writes the release `index.json` from `marketplace.json`, the tag, and a directory of archives.

mod archives;
mod catalog;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: marketplace-index <marketplace.json> <tag> <archive-dir> <index.json>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [catalog, tag, dir, out] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(Path::new(catalog), tag, Path::new(dir), Path::new(out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("marketplace-index: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(catalog_path: &Path, tag: &str, dir: &Path, out: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(catalog_path)
        .map_err(|e| format!("read {}: {e}", catalog_path.display()))?;
    let catalog: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("parse {}: {e}", catalog_path.display()))?;
    let archives = archives::collect(dir)?;
    let index = catalog::build_index(&catalog, tag, env!("CARGO_PKG_VERSION"), &archives)?;
    std::fs::write(out, catalog::render(&index))
        .map_err(|e| format!("write {}: {e}", out.display()))
}
