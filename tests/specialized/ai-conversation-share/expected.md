# How should a parser preserve code?

**User**

How can I preserve code in a static HTML extractor?

## Conversation

- **Assistant**

  Keep the retained DOM and normalize the code block separately.

  ```rust
  fn main() {
      println!("hello");
  }
  ```
  - Do not fetch the network.
  - Keep rendering lazy.
