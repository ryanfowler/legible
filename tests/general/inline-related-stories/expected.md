The migration starts by copying the active records into a stable staging area. Operators verify every count before they change the source system.

A second validation pass compares identifiers, timestamps, and checksums so that damaged records cannot enter the new store.

After validation succeeds, operators switch traffic in small groups and monitor error rates after each step.

The rollback package remains available until the final audit confirms that the new store contains every required record.
