# Backend

Code generation from IR to target-specific assembly, followed by assembling and linking.

## Architecture

```
IR Module → Codegen → Assembly text → Assembler → Object file → Linker → Executable
```

Each target architecture (x86-64, AArch64, RISC-V 64) has its own codegen module producing assembly text. Assembly and linking currently delegate to the system's GCC toolchain.

## Modules

- **common.rs** - Shared infrastructure: assembly output buffer, data section emission (globals, string literals, constants), `instruction_dest()` helper, assembler/linker config and invocation. Parameterized by a `PtrDirective` (`.quad`/`.xword`/`.dword`) for the only arch-specific data difference.
- **x86/** - x86-64 code generator (SysV AMD64 ABI)
- **arm/** - AArch64 code generator (AAPCS64)
- **riscv/** - RISC-V 64 code generator (standard calling convention)
- **mod.rs** - `Target` enum with methods for assembly generation, assembling, and linking

## Target Dispatch

`Target::generate_assembly()` dispatches to the correct architecture's codegen. `Target::assemble()` and `Target::link()` use shared logic parameterized by toolchain command name.

## Key Design Decisions

- All backends use a stack-based code generation strategy (no register allocator yet). Every IR value is stored in a stack slot. This produces correct but slow code.
- Data emission (globals, string literals, BSS) is shared across all backends since it uses identical GAS directives (except for the 64-bit pointer directive).
- The assembler/linker are thin wrappers around GCC invocations. These will eventually be replaced by a native ELF writer.
