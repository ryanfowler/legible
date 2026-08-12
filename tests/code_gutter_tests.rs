use legible::extract;
use std::fs;

fn fenced_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        if line.starts_with("```") {
            if let Some(mut block) = current.take() {
                block.push('\n');
                block.push_str(line);
                blocks.push(block);
            } else {
                current = Some(line.to_owned());
            }
        } else if let Some(block) = current.as_mut() {
            block.push('\n');
            block.push_str(line);
        }
    }
    blocks
}

#[test]
fn strips_line_number_gutters_from_imported_highlighter_fixtures() {
    let fixtures = [
        "tests/defuddle/code-blocks/chroma-linenums",
        "tests/defuddle/code-blocks/pygments-lineno",
        "tests/defuddle/code-blocks/rouge-linenums",
    ];

    for directory in fixtures {
        let source = fs::read_to_string(format!("{directory}/source.xfail.html"))
            .expect("fixture source exists");
        let expected = fs::read_to_string(format!("{directory}/expected.md"))
            .expect("fixture expected output exists");
        let markdown = extract(&source, Some("https://example.test/defuddle-fixture"))
            .expect("fixture extracts")
            .markdown();
        assert_eq!(
            fenced_blocks(&markdown),
            fenced_blocks(&expected),
            "{directory} produced different code blocks:\n{markdown}"
        );
    }
}
