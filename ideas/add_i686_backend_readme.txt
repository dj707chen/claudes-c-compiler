Add README.md for i686 backend
================================
Priority: LOW

The i686 backend at src/backend/i686/ is the only backend without a README.md.
All other backends (x86, arm, riscv) have one. Add a README documenting:
- The cdecl calling convention
- ILP32 type system differences
- Known limitations (most hardcoded 64-bit pointer sizes now fixed; remaining issues in project tests)
- Register allocation details
