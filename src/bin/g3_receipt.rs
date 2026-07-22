use std::path::PathBuf;

fn main() -> Result<(), String> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/receipts/g3_merecat_endpoint.html"));
    let receipt = merecat::remote_projection::render_g3_receipt()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, receipt)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}
