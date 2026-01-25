# x86-64 Backend

Code generation targeting the x86-64 (AMD64) architecture with System V ABI.

## Structure

- `codegen/` - Code generation implementation
  - `codegen.rs` - Main `X86Codegen` struct implementing the `ArchCodegen` trait. Handles instruction selection, register allocation (stack-based), calling convention, atomics, and varargs.
  - `inline_asm.rs` - x86 inline assembly template substitution and register formatting (AT&T syntax with `%` operand references).
  - `peephole.rs` - Post-codegen peephole optimizer. Operates on assembly text to eliminate redundant patterns from stack-based codegen (store/load forwarding, dead stores, compare-branch fusion, push/pop elimination).
  - `register.rs` - Register name definitions and utilities.
