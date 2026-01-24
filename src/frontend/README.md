# Frontend

Transforms C source text into a typed AST through four phases:

```
Source text → Preprocessor → Lexer → Parser → Sema → Typed AST
```

## Modules

- **preprocessor/** - `#include`, `#define`, `#if`/`#ifdef`, macro expansion, line splicing.
- **lexer/** - Tokenizes preprocessed source into `Token`s with source locations.
- **parser/** - Recursive descent parser producing a spanned AST. Split into focused sub-modules (expressions, types, statements, declarations, declarators). See `parser/README.md`.
- **sema/** - Semantic analysis: type checking, symbol table, `__builtin_*` mapping.

## Key Design Decisions

- The preprocessor runs as a text-to-text pass before lexing (not integrated into the lexer). This simplifies the architecture but means we lose original source locations within macros.
- The parser uses recursive descent (no parser generator), making error recovery and GCC extension support straightforward.
- Sema is permissive — it produces warnings but does not reject invalid programs. Most type inference is deferred to IR lowering (a known architectural weakness).

## What's Missing

- `#line`, `_Pragma`, `__has_attribute` in the preprocessor
- Full `_Atomic(type)` parsing (currently treated as underlying type)
- Proper error recovery in the parser (currently skips tokens on error)
