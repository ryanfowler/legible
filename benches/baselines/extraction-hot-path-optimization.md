# Extraction hot-path optimization

Baseline revision: `50ea687`

The comparison used the same machine and the Criterion baseline named
`repo-before`. The commands were:

```bash
cargo bench --bench pipeline -- --save-baseline repo-before
cargo bench --bench pipeline -- '^complex_pages/' --baseline repo-before
cargo bench --bench pipeline -- 'deeply_nested_document/8000' --baseline repo-before
cargo bench --bench smoke -- --baseline before
```

## Results

The complex extraction workload median times changed as follows:

| Workload | Median change |
|---|---:|
| Prose | -6.5% |
| Documentation | -4.4% |
| Footnotes | -5.8% |
| Highlighted code | -7.2% |
| Math | -2.6% |
| Media | -4.5% |
| Tables | -5.2% |
| Listing | -3.2% |
| Malformed HTML | -22.2% |
| Metadata | -5.5% |
| JSON-LD | -6.6% |

The 8,000-level parser workload improved by 7.9%. The smoke benchmark improved
large extraction by 10.0% and medium raw HTML-to-Markdown by 9.5%. Short smoke
measurements remained noisy. The full test suite produced the same output.

## Changes

- Specialized extractor discovery now checks each element's attributes once.
- Fragment snapshots reuse their traversal stack across cleanup phases.
- Terminal peripheral cleanup reuses its text buffer. It skips source shapes
  that cannot represent a terminal region. It also delays link-density work
  until marker evidence exists.
- Parser element-name callbacks clone only the namespace and local name. They
  do not clone the unused qualified-name prefix.
