//! Download an image from an http(s) URL without blocking the frame.
//!
//! The bytes land in the same content-addressed `images/` store as a file
//! drop. The address the user pasted is kept on the node so it can be copied
//! back (Midjourney CDN links, prompt pages that are themselves an image, …).

use std::io::Read;
use std::time::Duration;

/// Largest download we'll accept. Concept art is a few megabytes; this is
/// the ceiling that keeps a hostile URL from filling the disk.
const MAX_BYTES: usize = 64 * 1024 * 1024;

pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub filename: String,
}

/// Clipboard text that is nothing but one or more http(s) URLs, one per line.
/// A paragraph that merely mentions a URL is left for the file-path fallback.
pub fn parse_pasted_urls(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut urls = Vec::with_capacity(lines.len());
    for line in lines {
        let Some(url) = http_url(line) else {
            return Vec::new();
        };
        urls.push(url);
    }
    urls
}

/// Pull every http(s) URL out of a drop payload (`text/uri-list`, a single
/// URL, or several separated by newlines). `file:` lines are ignored; those
/// arrive as real paths through winit. Comments (`#…`) are skipped.
pub fn parse_drop_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in text.split(['\n', '\r', '\0']) {
        let line = line.trim().trim_matches(|c| c == '<' || c == '>');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(url) = http_url(line) {
            if !urls.iter().any(|have: &String| have == &url) {
                urls.push(url);
            }
        }
    }
    urls
}

fn http_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > 4096 || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    if rest.is_empty() || rest.contains("://") {
        return None;
    }
    Some(text.to_string())
}

/// Fetch `url` on a worker thread. Errors are short strings safe to show in
/// the status line.
pub fn download_image(url: &str) -> Result<Downloaded, String> {
    let Some(url) = http_url(url) else {
        return Err("not an http(s) image URL".to_string());
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent
        .get(&url)
        .header(
            "User-Agent",
            concat!("refcanvas/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|err| short_err(&err.to_string()))?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut reader = response.body_mut().as_reader();
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| format!("download failed: {err}"))?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BYTES {
            return Err("image is larger than 64 MB".to_string());
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    if bytes.is_empty() {
        return Err("URL returned an empty response".to_string());
    }
    if !is_image(&content_type, &bytes) {
        return Err("URL did not return an image".to_string());
    }
    let filename = filename_for(&url, &bytes, &content_type);
    Ok(Downloaded { bytes, filename })
}

fn is_image(content_type: &str, bytes: &[u8]) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    if ct.starts_with("text/") || ct == "application/json" || ct == "application/xhtml+xml" {
        return false;
    }
    sniff_ext(bytes).is_some() || ct.starts_with("image/")
}

fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("tiff")
    } else {
        None
    }
}

fn filename_for(url: &str, bytes: &[u8], content_type: &str) -> String {
    let ext = sniff_ext(bytes)
        .or_else(|| ext_from_type(content_type))
        .or_else(|| ext_from_url(url))
        .unwrap_or("png");
    let stem = url_stem(url).unwrap_or_else(|| "web-image".to_string());
    format!("{stem}.{ext}")
}

fn ext_from_type(content_type: &str) -> Option<&'static str> {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    match ct {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        _ => None,
    }
}

fn ext_from_url(url: &str) -> Option<&'static str> {
    let path = url.split('?').next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        "gif" => Some("gif"),
        "bmp" => Some("bmp"),
        "tif" | "tiff" => Some("tiff"),
        "tga" => Some("tga"),
        _ => None,
    }
}

fn url_stem(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let stem = stem.trim_matches('_').to_string();
    if stem.is_empty() { None } else { Some(stem) }
}

fn short_err(err: &str) -> String {
    let line = err.lines().next().unwrap_or(err);
    if line.len() > 160 {
        format!("{}…", &line[..160])
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_accepts_only_bare_image_urls() {
        assert_eq!(
            parse_pasted_urls("  https://cdn.example/a.png \n"),
            vec!["https://cdn.example/a.png".to_string()]
        );
        assert_eq!(
            parse_pasted_urls("https://a.test/1.png\nhttp://b.test/2.jpg"),
            vec![
                "https://a.test/1.png".to_string(),
                "http://b.test/2.jpg".to_string()
            ]
        );
        assert!(parse_pasted_urls("see https://a.test/1.png").is_empty());
        assert!(parse_pasted_urls("notion://page").is_empty());
    }

    #[test]
    fn drop_list_keeps_http_urls_and_skips_files() {
        let text = "\
# comment\r\n\
file:///tmp/a.png\r\n\
https://cdn.example/a.png\r\n\
https://cdn.example/a.png\r\n\
http://cdn.example/b.webp?x=1\r\n";
        assert_eq!(
            parse_drop_urls(text),
            vec![
                "https://cdn.example/a.png".to_string(),
                "http://cdn.example/b.webp?x=1".to_string()
            ]
        );
    }

    #[test]
    fn sniff_recognises_the_formats_the_importer_stores() {
        assert_eq!(sniff_ext(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0x00]), Some("jpg"));
        assert!(!is_image("text/html", b"<html>"));
        assert!(is_image("application/octet-stream", b"\x89PNG\r\n\x1a\n"));
        assert!(is_image("image/png", b"not really"));
    }
}
