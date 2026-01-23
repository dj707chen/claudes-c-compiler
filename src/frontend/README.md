# Frontend

The frontend transforms C source code into an AST suitable for semantic analysis.

## Pipeline

```
source text → preprocessor → lexer → parser → AST
                                                ↓
                                              sema (type checking)
```

## Modules

- **preprocessor/** - Macro expansion, `#include`, `#ifdef`/`#if` conditionals, builtin macros (`__FILE__`, `__LINE__`, etc.)
- **lexer/** - Tokenizes preprocessed source into a stream of `Token`s with source locations.
- **parser/** - Recursive descent parser producing a `TranslationUnit` AST. Handles declarations, statements, expressions, types.
- **sema/** - Semantic analysis: type checking, symbol table construction, `__builtin_*` function mapping.

## Key Design Decisions

- The preprocessor runs as a text-to-text pass before lexing (rather than being integrated into the lexer). This simplifies the architecture but means we lose original source locations within macros.
- The parser uses recursive descent (no parser generator), making error recovery and extension straightforward.
- Sema currently produces warnings but does not reject invalid programs; the compiler is permissive to maximize test coverage during development.
