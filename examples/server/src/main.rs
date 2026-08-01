use std::collections::HashSet;

use axum::{
    Router,
    extract::Query,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
};
use legible::parse;
use serde::Deserialize;

mod ssrf;

const DEFAULT_PORT: u16 = 3000;

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let app = Router::new()
        .route("/", get(index))
        .route("/article", get(article));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind to port");

    println!("Legible server running at http://localhost:{port}");

    axum::serve(listener, app)
        .await
        .expect("failed to start server");
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Deserialize)]
struct ArticleQuery {
    url: String,
}

async fn article(Query(query): Query<ArticleQuery>) -> impl IntoResponse {
    let url = query.url.trim();

    // Validate URL
    if url.is_empty() {
        return (StatusCode::BAD_REQUEST, Html(error_page("Please enter a URL")));
    }

    let parsed_url = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html(error_page("Invalid URL format. Please include http:// or https://")),
            );
        }
    };

    if !["http", "https"].contains(&parsed_url.scheme()) {
        return (
            StatusCode::BAD_REQUEST,
            Html(error_page("URL must use http or https")),
        );
    }

    // Fetch the HTML using SSRF-protected client
    let client = ssrf::build_safe_client();

    let html = match client.get(url).send().await {
        Ok(response) => {
            // Check content length before reading
            if let Some(len) = response.content_length()
                && len > ssrf::MAX_RESPONSE_SIZE
            {
                return (
                    StatusCode::BAD_REQUEST,
                    Html(error_page("Response too large (max 10 MB)")),
                );
            }

            if !response.status().is_success() {
                return (
                    StatusCode::BAD_GATEWAY,
                    Html(error_page(&format!(
                        "Failed to fetch URL: HTTP {}",
                        response.status()
                    ))),
                );
            }
            match response.text().await {
                Ok(text) => {
                    // Also check actual size after download
                    if text.len() as u64 > ssrf::MAX_RESPONSE_SIZE {
                        return (
                            StatusCode::BAD_REQUEST,
                            Html(error_page("Response too large (max 10 MB)")),
                        );
                    }
                    text
                }
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Html(error_page(&format!("Failed to read response: {e}"))),
                    );
                }
            }
        }
        Err(e) => {
            let msg = if e.is_timeout() {
                "Request timed out".to_string()
            } else if e.is_connect() {
                // Check for SSRF block from our resolver
                // Use debug format to get the full error chain including nested causes
                let err_debug = format!("{e:?}");
                if err_debug.contains("blocked IP address") {
                    "URL blocked: cannot access private or internal networks".to_string()
                } else {
                    "Failed to connect to server".to_string()
                }
            } else {
                format!("Failed to fetch URL: {e}")
            };
            return (StatusCode::BAD_GATEWAY, Html(error_page(&msg)));
        }
    };

    // Parse with Readability
    let article = match parse(&html, Some(url), None) {
        Ok(a) => a,
        Err(_) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Html(error_page(
                    "Could not extract readable content from this page",
                )),
            );
        }
    };

    // Sanitize the HTML content
    let sanitized_content = sanitize_html(&article.html());

    // Build the article page
    let byline_html = article
        .byline
        .as_ref()
        .map(|b| format!(r#"<p class="byline">{}</p>"#, html_escape(b)))
        .unwrap_or_default();

    let published_html = article
        .published_time
        .as_ref()
        .map(|p| format!(r#"<p class="published">{}</p>"#, html_escape(p)))
        .unwrap_or_default();


    let page = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title} - Legible Reader</title>
    <style>
        * {{
            box-sizing: border-box;
        }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, sans-serif;
            line-height: 1.7;
            color: #1a1a1a;
            background: #fafafa;
            margin: 0;
            padding: 20px;
        }}
        .container {{
            max-width: 680px;
            margin: 0 auto;
            background: #fff;
            padding: 40px;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        h1.title {{
            font-size: 2rem;
            line-height: 1.3;
            margin: 0 0 16px 0;
            color: #111;
        }}
        .byline {{
            color: #666;
            font-size: 0.95rem;
            margin: 0 0 8px 0;
        }}
        .published {{
            color: #888;
            font-size: 0.9rem;
            margin: 0 0 16px 0;
        }}
        hr {{
            border: none;
            border-top: 1px solid #eee;
            margin: 24px 0;
        }}
        .content {{
            font-size: 1.1rem;
        }}
        .content h1, .content h2, .content h3,
        .content h4, .content h5, .content h6 {{
            line-height: 1.3;
            margin-top: 1.5em;
            margin-bottom: 0.5em;
        }}
        .content p {{
            margin: 0 0 1em 0;
        }}
        .content img {{
            max-width: 100%;
            height: auto;
            border-radius: 4px;
        }}
        .content a {{
            color: #0066cc;
        }}
        .content blockquote {{
            border-left: 3px solid #ddd;
            padding-left: 16px;
            margin-left: 0;
            color: #555;
            font-style: italic;
        }}
        .content pre {{
            background: #f5f5f5;
            padding: 16px;
            overflow-x: auto;
            border-radius: 4px;
        }}
        .content code {{
            font-family: "SF Mono", Monaco, "Consolas", monospace;
            font-size: 0.9em;
            word-break: break-all;
        }}
        .content ul, .content ol {{
            padding-left: 24px;
        }}
        .content li {{
            margin-bottom: 0.5em;
        }}
        .content table {{
            width: 100%;
            border-collapse: collapse;
            margin: 1em 0;
        }}
        .content th, .content td {{
            border: 1px solid #ddd;
            padding: 8px 12px;
            text-align: left;
        }}
        .content th {{
            background: #f5f5f5;
        }}
        .back-link {{
            display: inline-block;
            padding: 12px 24px;
            background: #f0f0f0;
            color: #333;
            text-decoration: none;
            border-radius: 6px;
            font-size: 0.95rem;
        }}
        .back-link:hover {{
            background: #e5e5e5;
        }}
        .article-footer {{
            text-align: center;
            margin-top: 32px;
        }}
        .original-link {{
            display: block;
            margin-bottom: 16px;
            color: #666;
            font-size: 0.9rem;
        }}
        @media (max-width: 720px) {{
            body {{
                padding: 12px;
            }}
            .container {{
                padding: 24px;
            }}
            h1.title {{
                font-size: 1.5rem;
            }}
            .content {{
                font-size: 1rem;
            }}
        }}
    </style>
</head>
<body>
    <article class="container">
        <h1 class="title">{title}</h1>
        {byline}
        {published}
        <hr>
        <div class="content">
            {content}
        </div>
        <div class="article-footer">
            <a href="{url}" class="original-link" target="_blank" rel="noopener noreferrer">View original article</a>
            <a href="/" class="back-link">Read another article</a>
        </div>
    </article>
</body>
</html>"#,
        title = html_escape(&article.title),
        byline = byline_html,
        published = published_html,
        content = sanitized_content,
        url = html_escape(url),
    );

    (StatusCode::OK, Html(page))
}

fn sanitize_html(html: &str) -> String {
    let tags: HashSet<&str> = [
        "p", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre", "code", "ul", "ol", "li",
        "a", "img", "figure", "figcaption", "em", "strong", "b", "i", "br", "hr", "table", "thead",
        "tbody", "tr", "th", "td", "span", "div", "sup", "sub", "dl", "dt", "dd", "abbr", "cite",
        "time", "mark", "small", "del", "ins", "caption",
    ]
    .into_iter()
    .collect();

    let schemes: HashSet<&str> = ["http", "https"].into_iter().collect();

    ammonia::Builder::default()
        .tags(tags)
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(html)
        .to_string()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn error_page(message: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Error - Legible Reader</title>
    <style>
        * {{
            box-sizing: border-box;
        }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, sans-serif;
            line-height: 1.6;
            color: #1a1a1a;
            background: #fafafa;
            margin: 0;
            padding: 20px;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }}
        .container {{
            max-width: 480px;
            background: #fff;
            padding: 40px;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            text-align: center;
        }}
        h1 {{
            color: #c53030;
            font-size: 1.5rem;
            margin: 0 0 16px 0;
        }}
        p {{
            color: #555;
            margin: 0 0 24px 0;
        }}
        a {{
            display: inline-block;
            padding: 12px 24px;
            background: #333;
            color: #fff;
            text-decoration: none;
            border-radius: 6px;
        }}
        a:hover {{
            background: #444;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Something went wrong</h1>
        <p>{}</p>
        <a href="/">Try again</a>
    </div>
</body>
</html>"#,
        html_escape(message)
    )
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Legible Reader</title>
    <style>
        * {
            box-sizing: border-box;
        }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, sans-serif;
            line-height: 1.6;
            color: #1a1a1a;
            background: #fafafa;
            margin: 0;
            padding: 20px;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .container {
            max-width: 520px;
            width: 100%;
            background: #fff;
            padding: 48px;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            text-align: center;
        }
        h1 {
            font-size: 2rem;
            margin: 0 0 8px 0;
            color: #111;
        }
        .tagline {
            color: #666;
            margin: 0 0 32px 0;
        }
        form {
            display: flex;
            flex-direction: column;
            gap: 16px;
        }
        input[type="url"] {
            width: 100%;
            padding: 14px 16px;
            font-size: 1rem;
            border: 2px solid #e0e0e0;
            border-radius: 6px;
            transition: border-color 0.2s;
        }
        input[type="url"]:focus {
            outline: none;
            border-color: #333;
        }
        button {
            padding: 14px 24px;
            font-size: 1rem;
            font-weight: 500;
            background: #1a1a1a;
            color: #fff;
            border: none;
            border-radius: 6px;
            cursor: pointer;
            transition: background 0.2s;
        }
        button:hover {
            background: #333;
        }
        .info {
            margin-top: 24px;
            font-size: 0.9rem;
            color: #888;
        }
        @media (max-width: 560px) {
            .container {
                padding: 32px 24px;
            }
            h1 {
                font-size: 1.5rem;
            }
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>Legible Reader</h1>
        <p class="tagline">Extract clean, readable articles from any webpage</p>
        <form action="/article" method="GET">
            <input
                type="url"
                name="url"
                placeholder="https://example.com/article"
                required
                autofocus
            >
            <button type="submit">Extract Article</button>
        </form>
        <p class="info">
            Powered by Legible, a Rust port of Mozilla's Readability
        </p>
    </div>
</body>
</html>"#;
