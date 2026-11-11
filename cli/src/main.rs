//! Fetch a web page and write its extracted Markdown to standard output.

use std::io::{self, Read, Write};

use legible::{Extractor, ParseBudget};
use reqwest::blocking::Client;
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
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let response_content_length = response.content_length();
    let body = read_body(response, response_content_length)?;
    // Network HTML is untrusted input.
    let page = Extractor::builder()
        .parse_budget(ParseBudget::untrusted())
        .build()
        .extract_bytes(&body, Some(final_url.as_str()), content_type.as_deref())?;
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
    use legible::extract;

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
