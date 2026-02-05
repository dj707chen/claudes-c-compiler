Fix minor inaccuracies in ARM linker README.md:
- Line count estimates are slightly off vs actual code
- Relocation constant count in elf.rs says 28, actual is 26
- reloc.rs handles 40+ reloc types, not 30+
- Add mention of --gc-sections support
