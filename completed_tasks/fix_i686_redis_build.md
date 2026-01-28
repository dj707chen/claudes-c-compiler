# Fix i686 Redis Build

## Problem
Redis failed to build and run correctly on i686 due to two separate issues.

### Issue 1: PIC Global Address Crash (redis-cli)
`emit_global_addr` on i686 used GOT-relative addressing (`%ebx`-based) but never
set up `%ebx` as the GOT base register via `__x86.get_pc_thunk`. This caused
SIGSEGV in redis-cli's hiredis allocator function pointer access.

### Issue 2: mem2reg Constant Narrowing (redis-server)
`Expr::IntLiteral` always lowered integer literals as `IrConst::I64`, regardless
of target type. When mem2reg promoted a 32-bit alloca (e.g., `int exists`), the
I64 constant was pushed onto the def_stack. During phi elimination, this caused
64-bit copy operations for what should have been 32-bit variables, leaving the
upper 32 bits uninitialized on i686. Redis's `setKeyByLink` crashed because
`exists` (an int set to 0) was misread as non-zero due to garbage in the high bits.

## Fix

### PIC Global Address
Changed `emit_global_addr` in `src/backend/i686/codegen/codegen.rs` to always use
absolute addressing (`movl $name, %eax`) since we link with `-no-pie`.

### mem2reg Constant Narrowing
1. Added `IrConst::narrowed_to(ty)` method in `src/ir/ir.rs` to convert I64
   constants to smaller types (I8, I16, I32, Ptr).
2. Modified `rename_block` in `src/ir/mem2reg/mem2reg.rs` to narrow constants
   when processing Store instructions to match the alloca type.

## Files Changed
- `src/backend/i686/codegen/codegen.rs` - Absolute addressing for emit_global_addr
- `src/ir/ir.rs` - Added `IrConst::narrowed_to()` method
- `src/ir/mem2reg/mem2reg.rs` - Narrow constants in Store handling

## Tests Added
- `tests/i686-mem2reg-const-narrowing/` - Regression test for phi/exists bug
- `tests/i686-pic-global-addr-absolute/` - Regression test for PIC global addr

## Verification

```
Test Suite      x86                  ARM                  RISC-V               i686
----------------------------------------------------------------------
(10% sample)    2984/2990 (99.8%)    2858/2868 (99.7%)    2857/2859 (99.9%)    2731/2737 (99.8%)

Project         x86                  ARM                  RISC-V               i686
----------------------------------------------------------------------
zlib            PASS                 PASS                 PASS                 PASS
lua             PASS                 PASS                 PASS                 PASS
libsodium       PASS                 PASS                 PASS                 FAIL (pre-existing)
mquickjs        PASS                 PASS                 PASS                 FAIL (pre-existing)
libpng          PASS                 PASS                 PASS                 PASS
jq              PASS                 PASS                 PASS                 PASS
libjpeg         PASS                 PASS                 PASS                 PASS (NEW - was failing)
mbedtls         PASS                 PASS                 PASS                 FAIL (pre-existing)
libuv           PASS                 PASS                 PASS                 PASS
libffi          PASS                 PASS                 PASS                 FAIL (pre-existing)
musl            PASS                 PASS                 PASS                 FAIL (pre-existing)
tcc             PASS                 PASS                 PASS                 FAIL (pre-existing)
```

Redis i686 verification: PASS (server version, cli version, SET/GET roundtrip)

No regressions on any architecture. libjpeg additionally fixed on i686.
i686 projects: 6/12 passing (was 5/12).
