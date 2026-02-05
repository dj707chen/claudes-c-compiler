Fix incorrect phase count in x86 backend README.

The x86/README.md claims "15 passes in 8 phases" for the peephole optimizer,
but the actual code (peephole/passes/mod.rs) and the detailed peephole README
(x86/codegen/peephole/README.md) both describe 7 phases.
