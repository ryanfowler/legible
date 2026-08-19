This reference explains how to process records in a stable pipeline. Each stage uses an explicit input and produces a verified output.

The examples keep parsing, validation, and storage separate. This structure makes failures easy to diagnose.

## Validate each record

```
let valid = records.filter(validate);
```

Validation reports the rejected row. It keeps the original source available for inspection.

## Store the result

| Stage | Output |
| --- | --- |
| Parse | Record |
| Validate | Verified record |

The storage stage writes only verified records. It records the final count in the operation log.
