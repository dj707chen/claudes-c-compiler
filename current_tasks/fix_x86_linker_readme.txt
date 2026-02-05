Task: Fix x86 linker README for correctness and completeness

Review the x86 linker README.md against actual code, fixing:
- Incorrect line counts
- Wrong struct fields (missing fields in GlobalSymbol)
- Incorrect claims about struct locations (elf.rs vs linker_common.rs)
- Section sorting order description
- Missing .gcc_except_table and .eh_frame in name mapping
- Other factual errors found during review
