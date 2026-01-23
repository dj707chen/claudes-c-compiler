# IR Lowering

Translates the parsed AST into the IR representation. This is the largest module because it handles every C language construct.

## Module Organization

The lowering code is split into focused files:

- **lowering.rs** - Core: `Lowerer` struct, constructor, `lower()` entry point, IR emission helpers, function and global variable lowering
- **expr.rs** - Expression lowering: all binary/unary/postfix operators, function calls, casts, compound literals, sizeof, address-of, array subscripts, member access
- **stmt.rs** - Statement lowering: if/while/for/do-while/switch/case/goto/label/return/break/continue, local variable declarations
- **lvalue.rs** - L-value resolution and array address computation. Determines whether an expression refers to a memory location (for assignment, address-of, increment).
- **types.rs** - Type conversion helpers: TypeSpecifier→IrType mapping, sizeof computation, constant expression evaluation
- **structs.rs** - Struct/union layout computation, member offset resolution, struct base address resolution

## How Lowering Works

1. First pass: scan all top-level declarations to register globals and function signatures
2. Second pass: lower each function body and global initializer

Each function is lowered into basic blocks. The lowerer maintains:
- A scope stack tracking local variables and their alloca'd stack slots
- A struct layout cache mapping struct types to computed field offsets
- Break/continue target stacks for loop lowering
- A goto/label tracking system for forward references

## Key Invariants

- Every expression produces exactly one `Value` (an IR SSA value or constant)
- All local variables are allocated with `Alloca` and accessed via `Load`/`Store`
- Short-circuit evaluation (`&&`, `||`) uses control flow (conditional branches), not boolean operations
