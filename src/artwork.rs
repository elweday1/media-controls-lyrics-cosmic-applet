use std::sync::LazyLock;

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .expect("album art HTTP client")
});

pub async fn fetch(url: String) -> Result<Vec<u8>, String> {
    if url.starts_with("file://") {
        let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
        let path = parsed
            .to_file_path()
            .map_err(|_| "Invalid local album-art URL".to_owned())?;
        return tokio::fs::read(path)
            .await
            .map_err(|error| format!("Could not read album art: {error}"));
    }

    CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Could not download album art: {error}"))?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| error.to_string())
}

/// Center-crops the cover to a square RGBA thumbnail so it always fills its slot exactly.
pub fn thumbnail(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    const SIZE: u32 = 96;
    let image = image::load_from_memory(bytes).ok()?;
    let square = image.resize_to_fill(SIZE, SIZE, image::imageops::FilterType::Triangle).to_rgba8();
    Some((SIZE, SIZE, square.into_raw()))
}
