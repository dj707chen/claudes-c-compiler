# CCC - C Compiler Collection

A C compiler written from scratch in Rust, targeting x86-64, AArch64, and RISC-V 64.

## Status

**Basic compilation pipeline functional.** ~35% of x86-64 tests passing.

### Working Features
- Preprocessor (macros, conditionals, built-in headers)
- Recursive descent parser with typedef tracking
- Type-aware IR lowering and code generation
- Optimization passes (constant folding, DCE, GVN, algebraic simplification)
- Three backend targets with correct ABI handling

### Not Yet Implemented
- Full `#include` resolution (built-in headers only)
- Floating point arithmetic
- Native assembler/linker (currently delegates to GCC toolchain)
- SSA promotion (mem2reg)

## Building

```bash
cargo build --release
# Produces: target/release/ccc (x86), ccc-arm, ccc-riscv
```

## Usage

```bash
target/release/ccc -o output input.c       # x86-64
target/release/ccc-arm -o output input.c   # AArch64
target/release/ccc-riscv -o output input.c # RISC-V 64

# GCC-compatible flags: -S, -c, -E, -O0..3, -g, -D, -I
```

## Architecture

```
src/
  frontend/              C source → AST
    preprocessor/        Macro expansion, #include, #ifdef
    lexer/               Tokenization with source locations
    parser/              Recursive descent, produces AST
    sema/                Semantic analysis, symbol table

  ir/                    Target-independent SSA IR
    ir.rs                Core data structures (IrModule, Instructions, BasicBlock)
    lowering/            AST → alloca-based IR (split into expr/stmt/lvalue/types/structs)
    mem2reg/             SSA promotion (stub)

  passes/                Optimization: constant_fold, dce, gvn, simplify

  backend/               IR → assembly → object → executable
    common.rs            Shared data emission, assembler/linker invocation
    x86/codegen/         x86-64 instruction selection (SysV ABI)
    arm/codegen/         AArch64 instruction selection (AAPCS64)
    riscv/codegen/       RISC-V 64 instruction selection

  common/                Shared types (CType, IrType), symbol table, diagnostics
  driver/                CLI argument parsing, pipeline orchestration
```

Each subdirectory has its own README.md explaining the design and relationships.

## Testing

```bash
python3 /verify/verify_compiler.py --compiler target/release/ccc --arch x86 --ratio 10  # Quick (10%)
python3 /verify/verify_compiler.py --compiler target/release/ccc --arch x86              # Full suite
```
