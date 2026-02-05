Task: Fix peephole optimizer README accuracy

The peephole README has several inaccuracies:
- Missing Phase 7 (frame compaction) entirely
- combined_local_pass described as 6 patterns but actually has 7
- Pass count and phase count are wrong (14 passes/7 phases -> 15+/8 phases)
- File table missing frame_compact.rs
- Line counts in file table are stale
- mod.rs doc comment says 5 patterns instead of 7
