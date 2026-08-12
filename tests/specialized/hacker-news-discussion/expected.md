# [Show HN: A safe HTML extractor](https://example.com/legible)

75 points by maker 1 hour ago · 12 comments

The extractor keeps a stable document tree and produces useful Markdown without a browser.

## Comments

- **reader** · 45 minutes ago

  This design makes malformed input much easier to test.

  ```rust
  assert!(page.markdown().contains("extractor"));
  ```

  - **maker** · 30 minutes ago

    Yes. The arena also keeps node identifiers stable.
- **another** · 20 minutes ago

  It is useful that normal code normalization still runs.
