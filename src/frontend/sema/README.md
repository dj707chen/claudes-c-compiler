# Semantic Analysis

Performs semantic checks on the parsed AST before IR lowering.

## Files

- `sema.rs` - Main semantic analysis pass. Resolves typedefs, validates type specifiers, checks declarations, and normalizes the AST for lowering.
- `builtins.rs` - Builtin function signature definitions (e.g., `__builtin_va_start`, `__builtin_memcpy`). Provides type information for compiler builtins during semantic analysis.
- `type_context.rs` - `TypeContext` (module-level type state: struct layouts, typedefs, enum constants, ctype cache), `FunctionTypedefInfo`, and scope management. Created by sema, transferred to the lowerer.
- `type_checker.rs` - `ExprTypeChecker` for CType inference during semantic analysis. Uses SymbolTable, TypeContext, and FunctionInfo to infer expression types without depending on lowering state. Enables typeof(expr) resolution and will support future typed AST annotations.
- `const_eval.rs` - `SemaConstEval` for compile-time constant expression evaluation. Returns `IrConst` values supporting float literals, cast chains, sizeof/alignof, binary ops with type-aware signedness, offsetof patterns. Populates a `ConstMap` that the lowerer uses as an O(1) fast path before its own eval_const_expr.
