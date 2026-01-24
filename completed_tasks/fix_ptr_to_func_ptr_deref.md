# Fix pointer-to-function-pointer dereference generating no-op instead of load

## Summary

Dereferencing `int (**fpp)(int, int)` was treated as a no-op (like `*fp` for
direct function pointers) instead of emitting a Load instruction. This caused
segfaults when calling through dereferenced pointer-to-function-pointers.

## Root Cause

`build_full_ctype` absorbs extra pointer levels into the function return type,
making `Pointer(Pointer(Function(...)))` look like `Pointer(Function(...))`.
Three separate code paths checked CType shape to decide if a dereference was a
no-op, and all three were fooled by this ambiguity:

1. `is_function_pointer_deref` in `expr.rs` — returned `true` for ptr-to-fptr vars
2. `pointee_is_no_load` closure in `lower_deref` — included `Function(_)` as a no-load type
3. `emit_call_instruction`'s `Expr::Deref` arm in `expr_calls.rs` — had its own no-op detection

## Fix

Added `is_ptr_to_func_ptr: bool` flag to `VarInfo`, computed from the derived
declarator list (for local variables) or CType pointer depth (for parameters).
The flag is `true` when a declaration has exactly one `FunctionPointer` entry
preceded by 2+ consecutive `Pointer` entries in the derived declarator list.

Used this flag in all three locations:
- `is_function_pointer_deref`: early return `false` when flag is set
- `pointee_is_no_load`: removed `Function(_)` entirely; function pointer deref
  decisions are now handled by `is_function_pointer_deref`
- `emit_call_instruction` Deref arm: check flag before treating deref as no-op

## Files Changed

- `src/ir/lowering/definitions.rs` — added `is_ptr_to_func_ptr` to `VarInfo` and `DeclAnalysis`
- `src/ir/lowering/expr.rs` — updated `is_function_pointer_deref` and `pointee_is_no_load`
- `src/ir/lowering/expr_calls.rs` — updated `emit_call_instruction` Deref arm
- `src/ir/lowering/lowering.rs` — compute `is_ptr_to_func_ptr` for locals and parameters
- `tests/ptr-to-funcptr-deref-param/` — new regression test

## Test Results

- Before: 207/214 tests passing (96.7%)
- After: 215/219 tests passing (98.2%)
- Previously failing tests now passing: `fptr-ptr-param-deref-call`, `ptr-to-funcptr-deref-call`
- New regression test added and passing: `ptr-to-funcptr-deref-param`
- No regressions introduced; all existing function pointer tests continue to pass
