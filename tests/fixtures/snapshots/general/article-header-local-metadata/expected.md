# Stable record processing

A short guide to repeatable validation and storage.

The processor reads each source record and checks its required fields before it writes any output.

The validation stage reports a rejected record with its source position. This report lets the operator correct the input without changing valid records.

The storage stage writes the verified records in one transaction. It records the final count so another operator can confirm the result.
