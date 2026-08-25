A small cache with explicit expiry and predictable invalidation.

- [README](https://www.npmjs.com/package/steady-cache?activeTab=readme)

## Install

```
npm install steady-cache
```

## Usage

```
const cache = new Cache({ ttl: 60 });
cache.set('build', result);
cache.get('build');
```

The cache returns a value while its time-to-live remains valid. Call `clear()` after a deployment to remove old results.

## API

- `set(key, value)` stores a value.
- `get(key)` returns a current value.
- `clear()` removes all values.
