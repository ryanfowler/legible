Set the retry limit before you create the client. The client uses the same value for each request in the queue.

## Example

```toml
retry_limit = 3
timeout_ms = 5000
```

## Settings

| Name          | Meaning                  |
| ------------- | ------------------------ |
| `retry_limit` | Maximum request attempts |
| `timeout_ms`  | Timeout in milliseconds  |

Restart the worker after you change a configuration file.
