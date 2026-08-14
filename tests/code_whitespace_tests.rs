use legible::extract;

#[test]
fn extraction_preserves_code_whitespace_across_token_spans() {
    let html = concat!(
        "<main><article>",
        "<h1>Whitespace-sensitive example</h1>",
        "<p>This article explains a complete code example. The source uses syntax token spans, but every whitespace character remains meaningful to the program and its output.</p>",
        "<pre><code>",
        "<span>\n</span>",
        "<span>    first</span><span>  </span><span>\n</span>",
        "<span>\tsecond</span><span>\n</span>",
        "<span>  </span><span>\n</span>",
        "<span>\n</span>",
        "<span>  third  </span><span>\n\n</span>",
        "</code></pre>",
        "<p>The final paragraph confirms that the code sample is part of the article content and must remain available after extraction and cleanup.</p>",
        "</article></main>"
    );

    let page =
        extract(html, Some("https://example.test/code-whitespace")).expect("article extracts");
    let canonical_html = page.html();
    assert!(canonical_html.contains("<pre><code>"));
    assert!(
        !canonical_html.contains("<span>"),
        "canonical HTML removes syntax token implementation markup"
    );
    let markdown = page.markdown();
    let code = markdown
        .find("```")
        .map(|start| &markdown[start..])
        .expect("Markdown contains a fenced code block");
    let end = code[3..]
        .find("```")
        .map(|offset| offset + 6)
        .expect("fenced code block closes");

    assert_eq!(
        &code[..end],
        concat!(
            "```\n",
            "\n",
            "    first  \n",
            "\tsecond\n",
            "  \n",
            "\n",
            "  third  \n",
            "\n",
            "```"
        )
    );
}
