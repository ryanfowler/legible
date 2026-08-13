# Extraction quality baseline

> This curated corpus measures specific extraction cases. It is not an absolute measure of all web pages.

- Fixtures: 125
- Legible revision: `4ece3b68d34c239930f9ea6e0ba595c7e3f77f48`
- Defuddle revision: `npm:defuddle@0.19.2`

## Aggregate results

| Extractor | Content recall | Noise rejection | Structural fidelity | Metadata accuracy | Reference F1 | Reliability |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Legible | 0.94 | 0.91 | 0.90 | 0.70 | 0.98 | 0.98 |
| Defuddle | 0.86 | 0.93 | 0.72 | 0.52 | 1.00 | 0.97 |

## Results by category

| Category | Fixtures | Extractor | Content recall | Noise rejection | Structural fidelity | Metadata accuracy | Reference F1 | Reliability |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| api-reference | 7 | Legible | 1.00 | 0.93 | 1.00 | - | - | 1.00 |
| api-reference | 7 | Defuddle | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| application-markup | 14 | Legible | 0.55 | 0.91 | 0.31 | - | - | 0.86 |
| application-markup | 14 | Defuddle | 0.61 | 0.91 | 0.31 | - | - | 0.79 |
| blog-post | 6 | Legible | 1.00 | 0.67 | 1.00 | - | - | 1.00 |
| blog-post | 6 | Defuddle | 1.00 | 0.83 | 1.00 | - | - | 1.00 |
| code-heavy | 4 | Legible | 1.00 | 0.63 | 1.00 | - | - | 1.00 |
| code-heavy | 4 | Defuddle | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| discussion | 12 | Legible | 0.96 | 1.00 | 0.92 | - | - | 1.00 |
| discussion | 12 | Defuddle | 0.45 | 0.71 | 0.14 | - | - | 1.00 |
| inline-peripheral-ui | 7 | Legible | 1.00 | 0.57 | 1.00 | - | - | 1.00 |
| inline-peripheral-ui | 7 | Defuddle | 0.86 | 0.86 | 0.67 | - | - | 1.00 |
| legacy-html | 1 | Legible | 1.00 | 1.00 | 0.50 | - | - | 1.00 |
| legacy-html | 1 | Defuddle | 1.00 | 1.00 | 0.00 | - | - | 1.00 |
| link-index | 1 | Legible | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| link-index | 1 | Defuddle | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| long-form-essay | 6 | Legible | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| long-form-essay | 6 | Defuddle | 0.92 | 1.00 | 0.88 | - | - | 1.00 |
| media-embed | 12 | Legible | 0.85 | 1.00 | 0.86 | - | - | 1.00 |
| media-embed | 12 | Defuddle | 0.92 | 0.97 | 0.81 | - | - | 1.00 |
| news-article | 32 | Legible | 1.00 | 0.97 | 1.00 | 0.70 | - | 1.00 |
| news-article | 32 | Defuddle | 0.95 | 0.97 | 0.75 | 0.52 | - | 0.97 |
| product-support | 2 | Legible | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| product-support | 2 | Defuddle | 1.00 | 1.00 | 1.00 | - | - | 1.00 |
| responsive-duplicate | 3 | Legible | 1.00 | 1.00 | 1.00 | - | 1.00 | 1.00 |
| responsive-duplicate | 3 | Defuddle | 1.00 | 1.00 | 1.00 | - | 1.00 | 1.00 |
| social-thread | 2 | Legible | 1.00 | 0.83 | 1.00 | - | - | 1.00 |
| social-thread | 2 | Defuddle | 0.58 | 0.83 | 0.25 | - | - | 1.00 |
| technical-documentation | 16 | Legible | 1.00 | 0.94 | 1.00 | - | 0.93 | 1.00 |
| technical-documentation | 16 | Defuddle | 0.91 | 1.00 | 1.00 | - | 1.00 | 1.00 |

## Current gaps

- Largest comparator gaps: app-noscript-adjacent (-0.583), interview-transcript (-0.500), news-analysis-inline-promo (-0.333), code-heavy-line-numbers (-0.333), breaking-news-timeline (-0.333)
- Legible reliability failures: app-empty-shell, app-javascript-shell
- Defuddle reliability failures: app-access-barrier, app-empty-shell, app-javascript-shell, paywall-access-shell
