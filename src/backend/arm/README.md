# AArch64 Backend

Code generation targeting the AArch64 (ARM64) architecture with AAPCS64 calling convention.

## Structure

- `codegen/` - Code generation implementation
  - `codegen.rs` - Main `ArmCodegen` struct implementing the `ArchCodegen` trait. Handles instruction selection, stack frame management, calling convention (x0-x7 for integer args, d0-d7 for FP), atomics (LDXR/STXR exclusive access), and varargs.
  - `asm_emitter.rs` - `InlineAsmEmitter` trait implementation: constraint classification, scratch register allocation (GP and FP), operand loading/storing for inline asm.
  - `inline_asm.rs` - AArch64 inline assembly template substitution, register formatting (w/x/s/d modifiers), and exclusive load/store instruction helpers.
  - `intrinsics.rs` - NEON/SIMD intrinsic emission (SSE-equivalent 128-bit operations) and F128 (quad-precision) soft-float helpers via compiler-rt libcalls.
