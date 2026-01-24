# Backend

Code generation from IR to target-specific assembly, followed by assembling and linking.

## Architecture

```
IR Module → Codegen → Assembly text → Assembler → Object file → Linker → Executable
```

Each target architecture (x86-64, AArch64, RISC-V 64) implements the `ArchCodegen` trait, providing arch-specific instruction emission. The shared codegen framework handles instruction dispatch, function structure, and stack slot assignment.

## Modules

- **codegen_shared.rs** - Shared codegen framework: `ArchCodegen` trait, `CodegenState` (stack slots, alloca tracking), shared instruction dispatch loop, stack space calculation, parameter storage helpers. Also provides `CastKind` enum + `classify_cast()` for shared cast classification (Ptr normalization, F128 reduction, float/int/widen/narrow), and `FloatOp` enum + `classify_float_binop()` for shared float operation dispatch. This is where the common codegen logic lives. Adding a new IR instruction only requires changes here (dispatch) and in each arch's trait implementation.
- **common.rs** - Assembly output buffer, data section emission (globals, string literals, constants), assembler/linker config and invocation via external GCC toolchain.
- **x86/** - x86-64 codegen (SysV AMD64 ABI): `X86Codegen` implements `ArchCodegen`
- **arm/** - AArch64 codegen (AAPCS64): `ArmCodegen` implements `ArchCodegen`
- **riscv/** - RISC-V 64 codegen (standard calling convention): `RiscvCodegen` implements `ArchCodegen`
- **mod.rs** - `Target` enum for target dispatch

## Key Design Decisions

- **Trait-based deduplication**: The `ArchCodegen` trait in `codegen_shared.rs` eliminates the structural duplication between backends. The shared `generate_module()` function handles instruction dispatch, calling arch-specific methods for each operation. This prevents cross-backend inconsistency bugs and ensures new IR instructions are handled uniformly.
- **Default implementations via primitives**: Simple methods like `emit_branch`, `emit_cond_branch`, `emit_unreachable`, `emit_indirect_branch`, `emit_label_addr`, `emit_va_end`, and `emit_copy_i128` have default implementations in the trait. Backends provide small primitives (`jump_mnemonic()`, `trap_instruction()`, `emit_branch_nonzero()`, `emit_jump_indirect()`) and the defaults compose them. This avoids repeating identical control flow logic in each backend.
- **Cast classification**: The `classify_cast()` function captures the shared decision logic for type casts (Ptr normalization, F128 reduction, float<->int, widen/narrow) that all three backends need identically. Each backend then matches on `CastKind` to emit its arch-specific instructions. This prevents the three backends from drifting out of sync on cast handling.
- **Stack-based codegen**: All backends use a stack-based strategy (no register allocator yet). Each IR value gets a stack slot. Instructions load operands into a primary accumulator register (rax/x0/t0), perform the operation, and store the result back. This produces correct but slow code.
- **Arch-specific stack conventions**: x86 uses negative offsets from %rbp, ARM uses positive offsets from sp (with fp/lr at offset 0), RISC-V uses negative offsets from s0. The `calculate_stack_space_common` helper takes a closure to handle these differences.
- **Large offset handling**: ARM and RISC-V have limited immediate ranges. Their backends include helpers (`emit_store_to_sp`, `emit_load_from_s0`, etc.) that use scratch registers for large offsets.
- **Data emission is shared**: All backends use identical GAS directives for data sections, parameterized only by the 64-bit pointer directive (`.quad`/`.xword`/`.dword`).
- **Assembler/linker via GCC**: Currently delegates to the system's GCC toolchain. Will eventually be replaced by a native ELF writer.
