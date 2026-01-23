# Optimization Passes

SSA-based optimization passes that improve the IR before code generation.

## Available Passes

- **constant_fold.rs** - Evaluates constant expressions at compile time (e.g., `3 + 4` → `7`)
- **dce.rs** - Dead code elimination: removes instructions whose results are never used
- **gvn.rs** - Global value numbering: eliminates redundant computations (common subexpression elimination)
- **simplify.rs** - Algebraic simplification: identity removal (`x + 0` → `x`), strength reduction (`x * 2` → `x << 1`), boolean simplification

## Pass Pipeline

Passes are run in a fixed-point loop until no more changes occur (or a maximum iteration limit is reached). The pipeline runs: constant folding → simplification → GVN → DCE.

At `-O0`, no passes run. At `-O1` and above, all passes run.

## Adding New Passes

Each pass implements a function `fn run(module: &mut IrModule) -> bool` that returns `true` if it made any changes. Register new passes in `mod.rs::run_passes()`.
