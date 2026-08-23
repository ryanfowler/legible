A write-ahead log records each change before the database applies it. This order lets the database recover after an unexpected stop.

Our test splits the log at every valid record boundary. It confirms that recovery returns the last complete state.

The workload writes records and checkpoints the database at the same time. Each assertion checks committed data and verifies the complete file structure.

The corrected release completes every run without a failed assertion. The earlier release reproduces the failure under the same controlled workload.
