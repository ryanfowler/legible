# Website-shape fixtures

These fixtures are small, original documents that model markup patterns used by
popular websites. They are not copied pages. The content is synthetic so the
repository does not redistribute third-party text or private data.

Each fixture keeps the page chrome, content boundary, metadata, and one or two
site-specific structures that matter for extraction. The snapshot checks exact
Markdown and expected metadata, including the removal of rejected chrome.

## Verified layout anchors

The fixtures use stable or semantic anchors that were checked against public
HTML where available. They do not depend on every current CSS class.

| Fixture | Layout represented |
| --- | --- |
| `wikipedia-article` | MediaWiki `#mw-panel`, `#mw-content-text`, `.mw-parser-output`, and terminal `.navbox` content. |
| `github-readme` | A rendered blob under `.repository-content` with `article.markdown-body.entry-content.container-lg`. |
| `stackoverflow-question` | Question and answer bodies using Stack Overflow's `s-prose js-post-body` pattern. |
| `mdn-reference` | `main#content.layout__content`, a reference header, breadcrumbs, code, and documentation sections. |
| `medium-article` | Generic publisher article, author block, figure, newsletter, and recommendation chrome. Medium's anti-bot responses make stronger current DOM claims unreliable. |
| `reddit-thread` | The static old Reddit layout: `#siteTable`, `thing link`, `usertext-body md`, and nested comments. It uses an `old.reddit.com` URL intentionally. |
| `npm-package` | `main#main`, package tabs, `#tabpanel-readme`, `article`, `#readme`, and an accessible downloads panel. npm currently uses hashed presentation classes. |
| `arxiv-paper` | LaTeXML HTML under `.ltx_page_content`, `.ltx_document`, `.ltx_authors`, and `.ltx_section`. |
| `bun-docs` | Bun-style documentation layout with `main#main`, documentation navigation rails, `article#docs-content`, breadcrumbs, pagination, and an on-page index. |

A `url.txt` file sets the source URL used for relative-link resolution. The
runner uses it only for extraction and never fetches it. It compares the
expected Markdown and metadata.
