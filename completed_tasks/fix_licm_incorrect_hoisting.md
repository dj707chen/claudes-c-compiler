Task: Fix LICM incorrect load hoisting causing postgres initdb crash
Status: Complete
Branch: master

Problem:
The LICM (Loop-Invariant Code Motion) pass incorrectly hoisted loads
from allocas that were modified inside the loop through GEP-derived
pointers. This caused PostgreSQL's initdb to crash with "invalid
checkpoint record" / "PANIC: could not locate a valid checkpoint record"
at -O2.

Root cause:
When lowering struct field access, field at offset 0 (e.g., s.a) reuses
the alloca pointer directly, while non-zero offset fields (e.g., s.b) go
through a GEP instruction. The LICM alias analysis (analyze_loop_memory)
tracked stores by their direct pointer value ID. A store to
GEP(alloca, offset) only recorded the GEP's value ID in stored_allocas,
NOT the base alloca's ID. When is_load_hoistable checked whether the
alloca was in stored_allocas, it found no match and incorrectly
concluded the load was safe to hoist.

Diagnosis:
- postgres -O0: PASS
- postgres -O1: PASS (no LICM at O1)
- postgres -O2 with LICM disabled: PASS
- postgres -O2 with LICM enabled: FAIL
- postgres -O2 with only load hoisting disabled: PASS

Fix:
Disabled load hoisting entirely until proper field-sensitive alias
analysis is implemented. Also added build_base_pointer_map() to trace
GEP/Copy chains back to base allocas, improving store tracking in
analyze_loop_memory() as infrastructure for future re-enabling.

Files changed:
- src/passes/licm.rs: Disabled load hoisting, added base pointer map,
  updated store tracking, updated unit tests
- src/passes/README.md: Updated LICM description
- tests/licm-gep-store-alias/: Regression test for the bug
