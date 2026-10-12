use std::path::PathBuf;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Line {
    pub time: f64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct FetchResult {
    pub lines: Vec<Line>,
    pub source: String,
}

#[derive(Deserialize, Serialize)]
struct CacheEntry {
    lines: Vec<Line>,
    #[serde(default)]
    source: Value,
}

pub async fn fetch(
    title: String,
    artist: String,
    album: String,
    duration: f64,
    config: Config,
) -> Result<FetchResult, String> {
    let identity = format!(
        "{}\0{}\0{}\0{}",
        title.to_lowercase(),
        artist.to_lowercase(),
        album.to_lowercase(),
        duration.round() as i64
    );
    if config.caching_enabled {
        if let Some(entry) = read_cache(&identity).await {
            if !entry.lines.is_empty() {
                return Ok(FetchResult {
                    lines: entry.lines,
                    source: source_label(&entry.source),
                });
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "DankMaterialShell MusicLyrics/1.5.0 (ported to COSMIC; ",
            "https://github.com/Gasiyu/dms-plugin-musiclyrics)"
        ))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;

    if !config.navidrome_url.trim().is_empty()
        && !config.navidrome_user.is_empty()
        && !config.navidrome_password.is_empty()
    {
        if let Ok((lines, _plain_only)) = fetch_navidrome(&client, &config, &title).await {
            if !lines.is_empty() {
                save_cache(&identity, &lines, 1, config.caching_enabled).await;
                return Ok(FetchResult { lines, source: "Navidrome".into() });
            }
        }
    }

    if let Ok(Some(lines)) = fetch_musixmatch(&client, &title, &artist).await {
        if !lines.is_empty() {
            save_cache(&identity, &lines, 4, config.caching_enabled).await;
            return Ok(FetchResult { lines, source: "Musixmatch".into() });
        }
    }

    if let Ok(lines) = fetch_lrc_api(&client, &title, &artist, &album).await {
        if !lines.is_empty() {
            save_cache(&identity, &lines, 5, config.caching_enabled).await;
            return Ok(FetchResult { lines, source: "LRC API".into() });
        }
    }

    match fetch_lrclib(&client, &title, &artist, &album, duration).await {
        Ok((Some(lines), _)) if !lines.is_empty() => {
            save_cache(&identity, &lines, 2, config.caching_enabled).await;
            Ok(FetchResult { lines, source: "LRCLIB".into() })
        }
        Ok((_, true)) => Err("Only unsynced lyrics are available".to_owned()),
        Ok(_) => Err("No synced lyrics found".to_owned()),
        Err(error) => Err(error),
    }
}

fn source_label(source: &Value) -> String {
    match source.as_i64() {
        Some(1) => "Navidrome".into(),
        Some(2) => "LRCLIB".into(),
        Some(4) => "Musixmatch".into(),
        Some(5) => "LRC API".into(),
        _ => source.as_str().unwrap_or("Local cache").to_owned(),
    }
}

async fn fetch_navidrome(
    client: &reqwest::Client,
    config: &Config,
    title: &str,
) -> Result<(Vec<Line>, bool), String> {
    let mut search = navidrome_url(config, "search3")?;
    search.query_pairs_mut().append_pair("query", title);
    search.query_pairs_mut().append_pair("songCount", "5");
    search.query_pairs_mut().append_pair("albumCount", "0");
    search.query_pairs_mut().append_pair("artistCount", "0");
    let response: Value = get_retry(client, search)
        .await?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let song_value = &response["subsonic-response"]["searchResult3"]["song"];
    let songs = song_value
        .as_array()
        .cloned()
        .or_else(|| song_value.as_object().map(|_| vec![song_value.clone()]))
        .unwrap_or_default();
    let song = songs
        .iter()
        .find(|song| {
            song["title"]
                .as_str()
                .is_some_and(|found| found.eq_ignore_ascii_case(title))
        })
        .or_else(|| songs.first())
        .ok_or_else(|| "No Navidrome match".to_owned())?;
    let id = song["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| song["id"].as_i64().map(|id| id.to_string()))
        .ok_or_else(|| "Navidrome returned no song ID".to_owned())?;

    let mut url = navidrome_url(config, "getLyricsBySongId")?;
    url.query_pairs_mut().append_pair("id", &id);
    let response: Value = get_retry(client, url)
        .await?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let structured = &response["subsonic-response"]["lyricsList"]["structuredLyrics"];
    let candidates = structured
        .as_array()
        .cloned()
        .or_else(|| structured.as_object().map(|_| vec![structured.clone()]))
        .unwrap_or_default();
    let mut plain_only = false;
    for candidate in candidates {
        let lines = candidate["line"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|line| {
                Some(Line {
                    time: line["start"].as_f64()? / 1000.0,
                    text: line["value"].as_str().unwrap_or_default().to_owned(),
                })
            })
            .collect::<Vec<_>>();
        if candidate["synced"].as_bool().unwrap_or(false) && !lines.is_empty() {
            return Ok((lines, false));
        }
        plain_only |= !lines.is_empty();
    }
    Ok((Vec::new(), plain_only))
}

fn navidrome_url(config: &Config, endpoint: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/rest/{endpoint}",
        config.navidrome_url.trim().trim_end_matches('/')
    ))
    .map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("u", &config.navidrome_user)
        .append_pair("p", &config.navidrome_password)
        .append_pair("v", "1.16.1")
        .append_pair("c", "COSMICMusicLyrics")
        .append_pair("f", "json");
    Ok(url)
}

async fn fetch_musixmatch(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> Result<Option<Vec<Line>>, String> {
    for refresh in 0..2 {
        let token = if refresh == 0 {
            read_token().await
        } else {
            None
        };
        let token = match token {
            Some(token) => token,
            None => request_token(client).await?,
        };

        let mut url = reqwest::Url::parse(
            "https://apic-desktop.musixmatch.com/ws/1.1/matcher.track.get",
        )
        .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("q_track", title)
            .append_pair("q_artist", artist)
            .append_pair("page_size", "1")
            .append_pair("page", "1")
            .append_pair("app_id", "web-desktop-app-v1.0")
            .append_pair("usertoken", &token)
            .append_pair("t", &now_millis());
        let result: Value = musixmatch_get(client, url)
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let code = result["message"]["header"]["status_code"]
            .as_i64()
            .unwrap_or_default();
        if code == 401 || code == 402 {
            clear_token().await;
            if refresh == 0 {
                continue;
            }
            return Err(format!("Musixmatch authentication failed ({code})"));
        }
        let track = &result["message"]["body"]["track"];
        let Some(track_id) = track["track_id"].as_i64() else {
            return Ok(None);
        };
        if track["has_subtitles"].as_i64() != Some(1) {
            return Ok(None);
        }

        let mut url = reqwest::Url::parse(
            "https://apic-desktop.musixmatch.com/ws/1.1/track.subtitle.get",
        )
        .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("track_id", &track_id.to_string())
            .append_pair("subtitle_format", "lrc")
            .append_pair("app_id", "web-desktop-app-v1.0")
            .append_pair("usertoken", &token)
            .append_pair("t", &now_millis());
        let response: Value = musixmatch_get(client, url)
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        let code = response["message"]["header"]["status_code"]
            .as_i64()
            .unwrap_or_default();
        if code == 401 || code == 402 {
            clear_token().await;
            if refresh == 0 {
                continue;
            }
            return Err(format!("Musixmatch authentication failed ({code})"));
        }
        let text = response["message"]["body"]["subtitle"]["subtitle_body"]
            .as_str()
            .unwrap_or_default();
        let lines = parse_lrc(text);
        return Ok((!lines.is_empty()).then_some(lines));
    }
    Ok(None)
}

async fn request_token(client: &reqwest::Client) -> Result<String, String> {
    let mut url = reqwest::Url::parse(
        "https://apic-desktop.musixmatch.com/ws/1.1/token.get",
    )
    .map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("user_language", "en")
        .append_pair("app_id", "web-desktop-app-v1.0")
        .append_pair("t", &now_millis());
    let result: Value = musixmatch_get(client, url)
        .await?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let token = result["message"]["body"]["user_token"]
        .as_str()
        .filter(|token| !token.is_empty() && *token != "undefined")
        .ok_or_else(|| "Musixmatch token unavailable".to_owned())?
        .to_owned();
    if let Some(path) = token_path() {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(path, &token).await;
    }
    Ok(token)
}

async fn read_token() -> Option<String> {
    tokio::fs::read_to_string(token_path()?).await.ok()
}

async fn clear_token() {
    if let Some(path) = token_path() {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn token_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache/musicLyrics/musixmatch-token"))
}

async fn musixmatch_get(
    client: &reqwest::Client,
    url: reqwest::Url,
) -> Result<reqwest::Response, String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        let response = client
            .get(url.clone())
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/139.0.0.0 Safari/537.36",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .header(reqwest::header::ORIGIN, "https://www.musixmatch.com")
            .header(reqwest::header::REFERER, "https://www.musixmatch.com/")
            .send()
            .await;
        match response {
            Ok(response) => return response.error_for_status().map_err(|error| error.to_string()),
            Err(error) => {
                last_error = error.to_string();
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }
    }
    Err(last_error)
}

async fn fetch_lrc_api(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
) -> Result<Vec<Line>, String> {
    let mut params = vec![("artist", artist), ("title", title)];
    if !album.is_empty() {
        params.push(("album", album));
    }
    let url = reqwest::Url::parse_with_params("https://api.lrc.cx/lyrics", params)
        .map_err(|error| error.to_string())?;
    let body = get_retry(client, url)
        .await?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .text()
        .await
        .map_err(|error| error.to_string())?;
    Ok(parse_lrc(&body))
}

async fn fetch_lrclib(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    duration: f64,
) -> Result<(Option<Vec<Line>>, bool), String> {
    let mut url = reqwest::Url::parse("https://lrclib.net/api/get")
        .map_err(|error| error.to_string())?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("artist_name", artist)
            .append_pair("track_name", title);
        if !album.is_empty() {
            query.append_pair("album_name", album);
        }
        if duration > 0.0 {
            query.append_pair("duration", &duration.round().to_string());
        }
    }
    let response = get_retry(client, url).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok((None, false));
    }
    let result: Value = response
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    if let Some(lrc) = result["syncedLyrics"].as_str() {
        let lines = parse_lrc(lrc);
        if !lines.is_empty() {
            return Ok((Some(lines), false));
        }
    }
    Ok((None, result["plainLyrics"].as_str().is_some()))
}

async fn get_retry(
    client: &reqwest::Client,
    url: reqwest::Url,
) -> Result<reqwest::Response, String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        match client.get(url.clone()).send().await {
            Ok(response) => return Ok(response),
            Err(error) => {
                last_error = error.to_string();
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }
    }
    Err(last_error)
}

fn now_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn parse_lrc(contents: &str) -> Vec<Line> {
    let Ok(timestamp) = Regex::new(r"\[(\d{1,2}):(\d{2})[.:](\d{2,3})\]") else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for raw_line in contents.lines() {
        let stamps = timestamp
            .captures_iter(raw_line)
            .filter_map(|captures| {
                let minutes = captures.get(1)?.as_str().parse::<f64>().ok()?;
                let seconds = captures.get(2)?.as_str().parse::<f64>().ok()?;
                let fraction = captures.get(3)?.as_str();
                let fraction = fraction.parse::<f64>().ok()?
                    / if fraction.len() == 2 { 100.0 } else { 1000.0 };
                Some(minutes * 60.0 + seconds + fraction)
            })
            .collect::<Vec<_>>();
        let text = timestamp.replace_all(raw_line, "").trim().to_owned();
        for time in stamps {
            lines.push(Line {
                time,
                text: text.clone(),
            });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

fn cache_path(identity: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache/musicLyrics").join(format!(
        "{:08x}.json",
        fnv1a(identity)
    )))
}

fn fnv1a(value: &str) -> u32 {
    value.encode_utf16().fold(0x811c9dc5, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x01000193)
    })
}

async fn read_cache(identity: &str) -> Option<CacheEntry> {
    serde_json::from_slice(&tokio::fs::read(cache_path(identity)?).await.ok()?).ok()
}

async fn save_cache(identity: &str, lines: &[Line], source: i32, enabled: bool) {
    if !enabled {
        return;
    }
    let Some(path) = cache_path(identity) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
    }
    let entry = CacheEntry {
        lines: lines.to_vec(),
        source: Value::from(source),
    };
    if let Ok(data) = serde_json::to_vec(&entry) {
        let _ = tokio::fs::write(path, data).await;
    }
}

pub fn current_line(lines: &[Line], position: f64) -> Option<usize> {
    lines.iter().rposition(|line| position >= line.time)
}

pub fn visible_window(line_count: usize, selected: usize) -> std::ops::Range<usize> {
    let count = line_count.min(3);
    let selected = selected.min(line_count.saturating_sub(1));
    let start = selected.saturating_sub(1).min(line_count.saturating_sub(count));
    start..start + count
}


#[cfg(test)]
mod tests {
    use super::{current_line, parse_lrc, visible_window};

    #[test]
    fn parses_lrc_timestamps_and_selects_the_active_line() {
        let lines = parse_lrc("[00:01.25][00:02.500]Hello\n[00:03.00]World");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].time, 1.25);
        assert_eq!(current_line(&lines, 2.0), Some(0));
        assert_eq!(current_line(&lines, 3.0), Some(2));
    }

    #[test]
    fn visible_lyrics_are_limited_to_three_and_track_the_edges() {
        assert_eq!(visible_window(10, 0), 0..3);
        assert_eq!(visible_window(10, 5), 4..7);
        assert_eq!(visible_window(10, 9), 7..10);
    }
}
