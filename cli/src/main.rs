//! Fetch a web page and write its extracted Markdown to standard output.

use std::io::{self, Read, Write};

use encoding_rs::{Encoding, WINDOWS_1252};
use legible::extract;
use reqwest::blocking::Client;
use reqwest::header::HeaderValue;
use terminal_size::{Width, terminal_size};
use url::Url;

const MAX_LINE_WIDTH: usize = 100;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const USER_AGENT: &str = "legible-cli/0.5 (+https://github.com/ryanfowler/legible)";

fn main() {
    if let Err(error) = run() {
        eprintln!("legible: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os();
    let program = arguments.next().unwrap_or_else(|| "legible".into());
    let Some(input) = arguments.next() else {
        return Err(format!("usage: {} URL", program.to_string_lossy()).into());
    };
    if arguments.next().is_some() {
        return Err(format!("usage: {} URL", program.to_string_lossy()).into());
    }

    let input = input.into_string().map_err(|_| "URL must be valid UTF-8")?;
    let url = Url::parse(&input).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("URL must use http or https".into());
    }

    let client = Client::builder().user_agent(USER_AGENT).build()?;
    let response = client.get(url).send()?.error_for_status()?;
    let final_url = response.url().clone();
    let charset = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(content_type_charset);
    let response_content_length = response.content_length();
    let body = read_body(response, response_content_length)?;
    let html = decode_html(&body, charset.as_deref());
    let page = extract(&html, Some(final_url.as_str()))?;
    let width = terminal_width()
        .unwrap_or(MAX_LINE_WIDTH)
        .min(MAX_LINE_WIDTH);

    let mut output = io::BufWriter::new(io::stdout().lock());
    write_frontmatter(&mut output, page.metadata(), final_url.as_str())?;
    output.write_all(b"\n")?;
    page.markdown_builder()
        .max_line_width(width)
        .write_io(&mut output)?;
    output.flush()?;
    Ok(())
}

fn read_body<R: Read>(reader: R, content_length: Option<u64>) -> io::Result<Vec<u8>> {
    if content_length.is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("response body exceeds {MAX_RESPONSE_BYTES} bytes"),
        ));
    }

    let capacity = content_length
        .map(|length| length as usize)
        .unwrap_or(8192)
        .min(MAX_RESPONSE_BYTES);
    let mut body = Vec::with_capacity(capacity);
    reader
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("response body exceeds {MAX_RESPONSE_BYTES} bytes"),
        ));
    }
    Ok(body)
}

fn terminal_width() -> Option<usize> {
    // COLUMNS is useful when stdout is redirected or when a terminal emulator
    // does not expose its size through the process standard handles.
    if let Some(width) = std::env::var_os("COLUMNS")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&width| width > 0)
    {
        return Some(width);
    }

    terminal_size()
        .map(|(Width(width), _)| usize::from(width))
        .filter(|&width| width > 0)
}

fn content_type_charset(value: &HeaderValue) -> Option<String> {
    let value = value.to_str().ok()?;
    value.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.trim().split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches(['"', '\'']);
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn decode_html(bytes: &[u8], declared_charset: Option<&str>) -> String {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return encoding.decode_with_bom_removal(bytes).0.into_owned();
    }

    if declared_charset.is_none()
        && let Ok(text) = std::str::from_utf8(bytes)
    {
        return text.to_owned();
    }

    let encoding = declared_charset
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .or_else(|| html_charset(bytes).and_then(|label| Encoding::for_label(label.as_bytes())))
        .unwrap_or(WINDOWS_1252);
    encoding.decode_with_bom_removal(bytes).0.into_owned()
}

/// Finds a `charset` value in the initial part of an HTML document. HTML's
/// encoding sniffing algorithm only needs to inspect the beginning of a page.
/// Restrict the scan to meta elements so text, comments, and scripts cannot
/// change the response encoding.
fn html_charset(bytes: &[u8]) -> Option<String> {
    let sample = &bytes[..bytes.len().min(4096)];
    let mut cursor = 0;
    while cursor + 5 <= sample.len() {
        if sample[cursor..].starts_with(b"<!--") {
            let Some(comment_end) = find_bytes(sample, cursor + 4, b"-->") else {
                break;
            };
            cursor = comment_end + 3;
            continue;
        }
        if starts_with_tag(sample, cursor, b"script") || starts_with_tag(sample, cursor, b"style") {
            let Some(open_end) = find_tag_end(sample, cursor + 1) else {
                break;
            };
            let tag_name = if starts_with_tag(sample, cursor, b"script") {
                b"script".as_slice()
            } else {
                b"style".as_slice()
            };
            let closing = [b"</", tag_name].concat();
            let Some(close_start) = find_bytes(sample, open_end + 1, &closing) else {
                break;
            };
            cursor = close_start + closing.len();
            continue;
        }
        if sample[cursor] != b'<'
            || !sample[cursor + 1..cursor + 5].eq_ignore_ascii_case(b"meta")
            || !sample
                .get(cursor + 5)
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
        {
            cursor += 1;
            continue;
        }

        let tag_start = cursor + 5;
        let Some(tag_end) = find_tag_end(sample, tag_start) else {
            break;
        };
        if let Some(charset) = meta_charset(&sample[tag_start..tag_end]) {
            return Some(charset);
        }
        cursor = tag_end + 1;
    }
    None
}

fn starts_with_tag(bytes: &[u8], start: usize, name: &[u8]) -> bool {
    bytes.get(start) == Some(&b'<')
        && bytes
            .get(start + 1..start + 1 + name.len())
            .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
        && bytes
            .get(start + 1 + name.len())
            .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || start > bytes.len() || bytes.len() - start < needle.len() {
        return None;
    }
    (start..=bytes.len() - needle.len())
        .find(|&index| bytes[index..index + needle.len()].eq_ignore_ascii_case(needle))
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        match (quote, byte) {
            (Some(delimiter), byte) if byte == delimiter => quote = None,
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn meta_charset(attributes: &[u8]) -> Option<String> {
    let mut cursor = 0;
    let mut http_equiv = None;
    let mut content = None;
    while cursor < attributes.len() {
        while attributes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/')
        {
            cursor += 1;
        }
        let name_start = cursor;
        while attributes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/'))
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &attributes[name_start..cursor];
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let Some(&b'=') = attributes.get(cursor) else {
            continue;
        };
        cursor += 1;
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (value, next) = attribute_value(attributes, cursor);
        cursor = next;
        if name.eq_ignore_ascii_case(b"charset") {
            return nonempty_ascii_value(value);
        }
        if name.eq_ignore_ascii_case(b"http-equiv") {
            http_equiv = Some(value);
        } else if name.eq_ignore_ascii_case(b"content") {
            content = Some(value);
        }
    }

    if http_equiv.is_some_and(|value| value.eq_ignore_ascii_case(b"content-type")) {
        return content.and_then(content_charset);
    }
    None
}

fn attribute_value(bytes: &[u8], start: usize) -> (&[u8], usize) {
    let Some(&first) = bytes.get(start) else {
        return (&[], start);
    };
    if matches!(first, b'\'' | b'"') {
        let value_start = start + 1;
        let end = bytes[value_start..]
            .iter()
            .position(|&byte| byte == first)
            .map_or(bytes.len(), |offset| value_start + offset);
        return (&bytes[value_start..end], end.saturating_add(1));
    }
    let end = bytes[start..]
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(bytes.len(), |offset| start + offset);
    (&bytes[start..end], end)
}

fn nonempty_ascii_value(value: &[u8]) -> Option<String> {
    let value = value.strip_suffix(b"/").unwrap_or(value);
    (!value.is_empty()).then(|| String::from_utf8_lossy(value).into_owned())
}

fn content_charset(value: &[u8]) -> Option<String> {
    let marker = b"charset";
    if value.len() < marker.len() {
        return None;
    }
    let end = value.len() - marker.len();
    for index in 0..=end {
        if !value[index..index + marker.len()].eq_ignore_ascii_case(marker) {
            continue;
        }
        let mut cursor = index + marker.len();
        while value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if value.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        while value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (label, _) = attribute_value(value, cursor);
        if !label.is_empty() {
            return nonempty_ascii_value(label);
        }
    }
    None
}

fn write_frontmatter<W: Write>(
    output: &mut W,
    metadata: &legible::Metadata,
    source_url: &str,
) -> io::Result<()> {
    output.write_all(b"---\n")?;
    write_scalar(output, "title", metadata.title.as_deref())?;
    write_scalar(output, "description", metadata.description.as_deref())?;
    write_list(output, "authors", &metadata.authors)?;
    write_scalar(output, "site_name", metadata.site_name.as_deref())?;
    write_scalar(output, "canonical_url", metadata.canonical_url.as_deref())?;
    write_scalar(output, "image", metadata.image.as_deref())?;
    write_scalar(output, "favicon", metadata.favicon.as_deref())?;
    write_scalar(output, "published_time", metadata.published_time.as_deref())?;
    write_scalar(output, "modified_time", metadata.modified_time.as_deref())?;
    write_scalar(output, "language", metadata.language.as_deref())?;
    write_scalar(output, "direction", metadata.direction.as_deref())?;
    write_scalar(output, "section", metadata.section.as_deref())?;
    write_list(output, "tags", &metadata.tags)?;
    write_scalar(output, "source", Some(source_url))?;
    output.write_all(b"---\n")
}

fn write_scalar<W: Write>(output: &mut W, name: &str, value: Option<&str>) -> io::Result<()> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    writeln!(output, "{name}: {}", yaml_string(value))
}

fn write_list<W: Write>(output: &mut W, name: &str, values: &[String]) -> io::Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    writeln!(output, "{name}:")?;
    for value in values {
        writeln!(output, "  - {}", yaml_string(value))?;
    }
    Ok(())
}

fn yaml_string(value: &str) -> String {
    // A JSON string is also a valid YAML double-quoted scalar. Using the JSON
    // encoder handles quotes, backslashes, and control characters safely.
    serde_json::to_string(value).expect("strings are always JSON-serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_windows_1252() {
        assert_eq!(
            decode_html(b"<p>caf\xe9</p>", Some("windows-1252")),
            "<p>café</p>"
        );
    }

    #[test]
    fn detects_html_charset() {
        assert_eq!(
            html_charset(br#"<meta charset = 'windows-1252'>"#).as_deref(),
            Some("windows-1252")
        );
    }

    #[test]
    fn ignores_short_invalid_html() {
        assert_eq!(html_charset(&[0xff]), None);
    }

    #[test]
    fn ignores_charset_in_scripts() {
        let html =
            br#"<script>const value = "<meta charset=utf-8>";</script><meta charset=windows-1252>"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn ignores_meta_in_comments() {
        let html = br#"<!-- <meta charset=utf-8> --><meta charset=windows-1252>"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn detects_http_equiv_charset() {
        let html = br#"<meta http-equiv=content-type content="text/html; charset=windows-1252">"#;
        assert_eq!(html_charset(html).as_deref(), Some("windows-1252"));
    }

    #[test]
    fn detects_unquoted_content_charset() {
        let html = br#"<meta http-equiv=content-type content=text/html;charset=shift_jis>"#;
        assert_eq!(html_charset(html).as_deref(), Some("shift_jis"));
    }

    #[test]
    fn rejects_oversized_response_bodies() {
        let body = vec![0; MAX_RESPONSE_BYTES + 1];
        let error = read_body(std::io::Cursor::new(body), None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn frontmatter_quotes_values() {
        let page = extract(
            "<title>A: title</title><main><p>Content.</p></main>",
            Some("https://example.com"),
        )
        .unwrap();
        let mut output = Vec::new();
        write_frontmatter(&mut output, page.metadata(), "https://example.com").unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("title: \"A: title\""));
        assert!(output.contains("source: \"https://example.com\""));
    }
}
