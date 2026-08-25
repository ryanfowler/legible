[rust](https://stackoverflow.com/tags/rust) [caching](https://stackoverflow.com/tags/caching)

How should a build tool design a cache key when source files and compiler options can change?

The key must be stable across machines but must not reuse an output after a compiler change.

asked by Example User

## Answer

Include a digest of the source inputs, compiler version, target platform, and relevant options.

```
key = hash(source, compiler, target, options)
```

answered by Another User
