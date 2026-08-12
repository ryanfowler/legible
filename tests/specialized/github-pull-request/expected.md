# Preserve stable node IDs

**alice** · Aug 9, 2026

This change keeps identifiers stable after HTML tree repair.

- Keep arena allocation private.
- Use preorder snapshots before mutation.

## Discussion

### reviewer · Aug 10, 2026

The traversal rule looks correct.

```
for node in snapshot {
    normalize(node);
}
```
