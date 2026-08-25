Build Cache stores reusable outputs so a project can finish repeated builds faster.

## Quick start

```bash
cache init ./build
cache run make test
```

## Configuration

| Option | Purpose |
| --- | --- |
| directory | Location of cached outputs |
| retention | Maximum cache age |

Use a stable directory in continuous integration and clear it after a toolchain upgrade.
