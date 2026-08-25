# [How do you test a cache?](https://old.reddit.com/r/programming/comments/example)

**Example Developer**

What tests catch stale entries when a cache is shared by several workers?

I want to verify expiry, invalidation, and concurrent reads without making the test depend on timing.


## Comments

- **Helpful Developer**

  Use a fake clock and an in-memory store. Advance the clock explicitly, then assert the value before and after expiry.
- **Second Developer**

  Also test that two workers observe the same version after invalidation.
