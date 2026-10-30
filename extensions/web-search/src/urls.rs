//! URL cleanup: tracking parameters, titles from paths, and junk file links.

use reqwest::Url;

fn is_youtube(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ["youtube.com", "youtu.be"]
        .iter()
        .any(|base| host == *base || host.ends_with(&format!(".{base}")))
}

const TRACKING_PARAMS: [&str; 6] = [
    "ref",
    "referral_type",
    "openLinerExtension",
    "_clear",
    "lang",
    "api-mode",
];

/// Drops the fragment and tracking parameters (`utm_*`, `ref`, ...) outside YouTube.
pub fn normalize_search_url(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    parsed.set_fragment(None);
    if !is_youtube(parsed.host_str().unwrap_or_default()) {
        let kept: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(name, _)| {
                !TRACKING_PARAMS.contains(&name.as_ref())
                    && !name.to_lowercase().starts_with("utm_")
            })
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        if kept.is_empty() {
            parsed.set_query(None);
        } else {
            parsed.query_pairs_mut().clear().extend_pairs(kept);
        }
    }
    parsed.to_string()
}

pub fn title_from_url(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    let last = parsed
        .path_segments()
        .and_then(|segments| segments.rev().find(|s| !s.is_empty()));
    match (last, parsed.host_str()) {
        (Some(segment), _) => segment.to_owned(),
        (None, Some(host)) if !host.is_empty() => host.to_owned(),
        _ => url.to_owned(),
    }
}

const JUNK_SUFFIXES: [&str; 20] = [
    ".gz", ".zip", ".tgz", ".tar", ".woff", ".woff2", ".ttf", ".otf", ".eot", ".webm", ".mp4",
    ".mp3", ".wav", ".eps", ".sql", ".csv", ".xls", ".xlsx", ".ppt", ".pptx",
];

pub fn is_junk_url(url: Option<&str>) -> bool {
    let Some(url) = url else { return true };
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let path = percent_decode(parsed.path()).to_lowercase();
    JUNK_SUFFIXES.iter().any(|suffix| path.ends_with(suffix))
        || path == "/%"
        || path.ends_with("/%")
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(byte) if bytes[i] == b'%' => {
                out.push(byte);
                i += 3;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tracking_params_and_fragments() {
        let url = "https://a.com/p?utm_source=x&id=2&ref=y#top";
        assert_eq!(normalize_search_url(url), "https://a.com/p?id=2");
        assert_eq!(
            normalize_search_url("https://a.com/p?utm_x=1"),
            "https://a.com/p"
        );
        let video = "https://www.youtube.com/watch?v=abc&utm_source=x";
        assert_eq!(normalize_search_url(video), video);
    }

    #[test]
    fn titles_and_junk() {
        assert_eq!(title_from_url("https://a.com/docs/page/"), "page");
        assert_eq!(title_from_url("https://a.com/"), "a.com");
        assert!(is_junk_url(Some("https://a.com/file.ZIP")));
        assert!(is_junk_url(Some("https://a.com/x/%25")));
        assert!(!is_junk_url(Some("https://a.com/page")));
    }
}
