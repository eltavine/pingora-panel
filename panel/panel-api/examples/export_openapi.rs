#![forbid(unsafe_code)]

use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&panel_api::ApiDoc::openapi())?
    );
    Ok(())
}
