# How should a parser preserve code?

**Ada** · August 13, 2026

I want a static extractor to preserve code and useful structure.

```rust
fn main() {
    println!("hello");
}
```


## Replies

- **Ben** · August 13, 2026

  > A static extractor should preserve useful structure.

  Yes. The code block must keep its indentation.
