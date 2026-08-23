# Parser drops code

**alice** · Aug 10, 2026

The parser drops indentation from this example:

```rust
fn main() {
    println!("hello");
}
```

> Whitespace is part of the example.


## Discussion

- **bob** · Aug 11, 2026

  I can reproduce this with nested token spans.
- **alice** · Aug 12, 2026

  The new normalization pass fixes it.
