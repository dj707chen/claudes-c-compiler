Task: Fix ARM linker README.md for correctness and accuracy

Issues found:
1. Public entry point signature is wrong (shows old `link()` instead of `link_builtin()`)
2. CRT discovery section describes logic that moved to common.rs
3. Line counts are inaccurate (mod.rs is 3,512 not ~3,370; total is 4,127 not ~3,985)
4. Claims register_symbols/merge_sections/allocate_common delegate to linker_common but they're local
5. OutputSection described as "shared type from linker_common" but it's defined locally
6. resolve_sym signature/pseudocode doesn't match actual code (no bss_addr parameter)
7. Missing .eh_frame_hdr from memory layout diagram
8. Missing PT_GNU_EH_FRAME from program headers
9. Missing features: --defsym, --export-dynamic, thin archives, shared library loading
10. Overview "roughly 3,500 lines" should be updated
