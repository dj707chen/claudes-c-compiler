Reviewing and correcting the i686 linker README.md for factual accuracy.

Key issues found:
- Archive extraction incorrectly described as "all members" when code does demand-driven extraction
- Missing documentation for TLS, IFUNC, COMDAT, thin archives, --defsym, text relocations
- Missing PT_TLS and PT_GNU_EH_FRAME program headers from table
- Section merging table inaccurate for .init/.fini (kept separate, not merged into .text)
- GOT reserved entry count wrong (1, not shown clearly)
- .eh_frame_hdr not documented
