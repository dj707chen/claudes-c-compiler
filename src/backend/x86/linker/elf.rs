/// ELF file parsing for the x86-64 linker.
///
/// Reads ELF relocatable object files (.o) and shared libraries (.so),
/// extracting sections, symbols, and relocations needed for linking.
///
/// Shared ELF constants and helpers are imported from `crate::backend::elf`.

pub use crate::backend::elf::{
    ELF_MAGIC, ELFCLASS64, ELFDATA2LSB, ET_REL, ET_EXEC, ET_DYN, EM_X86_64,
    SHT_NULL, SHT_PROGBITS, SHT_SYMTAB, SHT_STRTAB, SHT_RELA,
    SHT_DYNAMIC, SHT_NOBITS, SHT_REL, SHT_DYNSYM, SHT_GROUP,
    SHF_WRITE, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_EXCLUDE,
    STB_LOCAL, STB_GLOBAL, STB_WEAK,
    STT_OBJECT, STT_FUNC, STT_SECTION, STT_FILE, STT_TLS,
    SHN_UNDEF, SHN_ABS, SHN_COMMON,
    PT_LOAD, PT_DYNAMIC, PT_INTERP, PT_PHDR, PT_TLS, PT_GNU_STACK,
    PF_X, PF_W, PF_R,
    DT_NULL, DT_NEEDED, DT_PLTRELSZ, DT_PLTGOT, DT_STRTAB,
    DT_SYMTAB, DT_RELA, DT_RELASZ, DT_RELAENT, DT_STRSZ, DT_SYMENT,
    DT_JMPREL, DT_PLTREL, DT_GNU_HASH,
    read_u16, read_u32, read_u64, read_i64, read_cstr,
    parse_archive_members, parse_linker_script,
};

// x86-64 relocation types
pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOT32: u32 = 3;
pub const R_X86_64_PLT32: u32 = 4;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_GOTTPOFF: u32 = 22;
pub const R_X86_64_TPOFF32: u32 = 23;
pub const R_X86_64_PC64: u32 = 24;
pub const R_X86_64_GOTPCRELX: u32 = 41;
pub const R_X86_64_REX_GOTPCRELX: u32 = 42;

// x86-64 specific DT_* constants not in shared module
pub const DT_SONAME: i64 = 14;
pub const DT_DEBUG: i64 = 21;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_FLAGS: i64 = 30;
pub const DT_VERSYM: i64 = 0x6ffffff0;
pub const DT_VERNEED: i64 = 0x6ffffffe;
pub const DT_VERNEEDNUM: i64 = 0x6fffffff;
pub const DT_RELACOUNT: i64 = 0x6ffffff9;
pub const DF_BIND_NOW: i64 = 0x8;

/// Parsed ELF section header
#[derive(Debug, Clone)]
pub struct SectionHeader {
    pub name_idx: u32,
    pub name: String,
    pub sh_type: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}

/// Parsed ELF symbol
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name_idx: u32,
    pub name: String,
    pub info: u8,
    pub other: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

impl Symbol {
    pub fn binding(&self) -> u8 {
        self.info >> 4
    }

    pub fn sym_type(&self) -> u8 {
        self.info & 0xf
    }

    pub fn visibility(&self) -> u8 {
        self.other & 0x3
    }

    pub fn is_undefined(&self) -> bool {
        self.shndx == SHN_UNDEF
    }

    pub fn is_global(&self) -> bool {
        self.binding() == STB_GLOBAL
    }

    pub fn is_weak(&self) -> bool {
        self.binding() == STB_WEAK
    }

    pub fn is_local(&self) -> bool {
        self.binding() == STB_LOCAL
    }
}

/// Parsed ELF relocation with addend
#[derive(Debug, Clone)]
pub struct Rela {
    pub offset: u64,
    pub sym_idx: u32,
    pub rela_type: u32,
    pub addend: i64,
}

/// Parsed ELF object file
#[derive(Debug)]
pub struct ElfObject {
    pub sections: Vec<SectionHeader>,
    pub symbols: Vec<Symbol>,
    pub section_data: Vec<Vec<u8>>,
    /// Relocations indexed by the section they apply to
    pub relocations: Vec<Vec<Rela>>,
    pub source_name: String,
}

/// Parsed dynamic symbol from a shared library
#[derive(Debug, Clone)]
pub struct DynSymbol {
    pub name: String,
    pub info: u8,
    pub value: u64,
    pub size: u64,
}

impl DynSymbol {
    pub fn sym_type(&self) -> u8 {
        self.info & 0xf
    }
}


/// Parse an ELF relocatable object file (.o)
pub fn parse_object(data: &[u8], source_name: &str) -> Result<ElfObject, String> {
    if data.len() < 64 {
        return Err(format!("{}: file too small for ELF header", source_name));
    }

    // Validate ELF magic
    if data[0..4] != ELF_MAGIC {
        return Err(format!("{}: not an ELF file", source_name));
    }
    if data[4] != ELFCLASS64 {
        return Err(format!("{}: not 64-bit ELF", source_name));
    }
    if data[5] != ELFDATA2LSB {
        return Err(format!("{}: not little-endian ELF", source_name));
    }

    let e_type = read_u16(data, 16);
    if e_type != ET_REL {
        return Err(format!("{}: not a relocatable object (type={})", source_name, e_type));
    }

    let e_machine = read_u16(data, 18);
    if e_machine != EM_X86_64 {
        return Err(format!("{}: not x86-64 (machine={})", source_name, e_machine));
    }

    let e_shoff = read_u64(data, 40) as usize;
    let e_shentsize = read_u16(data, 58) as usize;
    let e_shnum = read_u16(data, 60) as usize;
    let e_shstrndx = read_u16(data, 62) as usize;

    if e_shoff == 0 || e_shnum == 0 {
        return Err(format!("{}: no section headers", source_name));
    }

    // Parse section headers
    let mut sections = Vec::with_capacity(e_shnum);
    for i in 0..e_shnum {
        let off = e_shoff + i * e_shentsize;
        if off + e_shentsize > data.len() {
            return Err(format!("{}: section header {} out of bounds", source_name, i));
        }
        sections.push(SectionHeader {
            name_idx: read_u32(data, off),
            name: String::new(), // filled in below
            sh_type: read_u32(data, off + 4),
            flags: read_u64(data, off + 8),
            addr: read_u64(data, off + 16),
            offset: read_u64(data, off + 24),
            size: read_u64(data, off + 32),
            link: read_u32(data, off + 40),
            info: read_u32(data, off + 44),
            addralign: read_u64(data, off + 48),
            entsize: read_u64(data, off + 56),
        });
    }

    // Read section name string table
    if e_shstrndx < sections.len() {
        let shstrtab = &sections[e_shstrndx];
        let strtab_off = shstrtab.offset as usize;
        let strtab_size = shstrtab.size as usize;
        if strtab_off + strtab_size <= data.len() {
            let strtab_data = &data[strtab_off..strtab_off + strtab_size];
            for sec in &mut sections {
                sec.name = read_cstr(strtab_data, sec.name_idx as usize);
            }
        }
    }

    // Read section data
    let mut section_data = Vec::with_capacity(e_shnum);
    for sec in &sections {
        if sec.sh_type == SHT_NOBITS || sec.size == 0 {
            section_data.push(Vec::new());
        } else {
            let start = sec.offset as usize;
            let end = start + sec.size as usize;
            if end > data.len() {
                return Err(format!("{}: section '{}' data out of bounds", source_name, sec.name));
            }
            section_data.push(data[start..end].to_vec());
        }
    }

    // Find symbol table and its string table
    let mut symbols = Vec::new();
    for i in 0..sections.len() {
        if sections[i].sh_type == SHT_SYMTAB {
            let strtab_idx = sections[i].link as usize;
            let strtab_data = if strtab_idx < section_data.len() {
                &section_data[strtab_idx]
            } else {
                continue;
            };
            let sym_data = &section_data[i];
            let sym_count = sym_data.len() / 24; // sizeof(Elf64_Sym) = 24
            for j in 0..sym_count {
                let off = j * 24;
                if off + 24 > sym_data.len() {
                    break;
                }
                let name_idx = read_u32(sym_data, off);
                symbols.push(Symbol {
                    name_idx,
                    name: read_cstr(strtab_data, name_idx as usize),
                    info: sym_data[off + 4],
                    other: sym_data[off + 5],
                    shndx: read_u16(sym_data, off + 6),
                    value: read_u64(sym_data, off + 8),
                    size: read_u64(sym_data, off + 16),
                });
            }
            break;
        }
    }

    // Parse relocations - index by the section they apply to
    let mut relocations = vec![Vec::new(); e_shnum];
    for i in 0..sections.len() {
        if sections[i].sh_type == SHT_RELA {
            let target_sec = sections[i].info as usize;
            let rela_data = &section_data[i];
            let rela_count = rela_data.len() / 24; // sizeof(Elf64_Rela) = 24
            let mut relas = Vec::with_capacity(rela_count);
            for j in 0..rela_count {
                let off = j * 24;
                if off + 24 > rela_data.len() {
                    break;
                }
                let r_info = read_u64(rela_data, off + 8);
                relas.push(Rela {
                    offset: read_u64(rela_data, off),
                    sym_idx: (r_info >> 32) as u32,
                    rela_type: (r_info & 0xffffffff) as u32,
                    addend: read_i64(rela_data, off + 16),
                });
            }
            if target_sec < relocations.len() {
                relocations[target_sec] = relas;
            }
        }
    }

    Ok(ElfObject {
        sections,
        symbols,
        section_data,
        relocations,
        source_name: source_name.to_string(),
    })
}

/// Extract dynamic symbols from a shared library (.so) file.
///
/// Reads the .dynsym section to find exported symbols.
pub fn parse_shared_library_symbols(data: &[u8], lib_name: &str) -> Result<Vec<DynSymbol>, String> {
    if data.len() < 64 {
        return Err(format!("{}: file too small for ELF header", lib_name));
    }
    if data[0..4] != ELF_MAGIC {
        return Err(format!("{}: not an ELF file", lib_name));
    }
    if data[4] != ELFCLASS64 || data[5] != ELFDATA2LSB {
        return Err(format!("{}: not 64-bit little-endian ELF", lib_name));
    }

    let e_type = read_u16(data, 16);
    if e_type != ET_DYN {
        return Err(format!("{}: not a shared library (type={})", lib_name, e_type));
    }

    let e_shoff = read_u64(data, 40) as usize;
    let e_shentsize = read_u16(data, 58) as usize;
    let e_shnum = read_u16(data, 60) as usize;
    let _e_shstrndx = read_u16(data, 62) as usize;

    if e_shoff == 0 || e_shnum == 0 {
        return Err(format!("{}: no section headers", lib_name));
    }

    // Parse section headers
    let mut sections = Vec::with_capacity(e_shnum);
    for i in 0..e_shnum {
        let off = e_shoff + i * e_shentsize;
        if off + e_shentsize > data.len() {
            break;
        }
        sections.push((
            read_u32(data, off + 4),  // sh_type
            read_u64(data, off + 24), // offset
            read_u64(data, off + 32), // size
            read_u32(data, off + 40), // link
        ));
    }

    // Find .dynsym and its string table
    let mut symbols = Vec::new();
    for i in 0..sections.len() {
        let (sh_type, offset, size, link) = sections[i];
        if sh_type == SHT_DYNSYM {
            // Get the dynamic string table
            let strtab_idx = link as usize;
            if strtab_idx >= sections.len() {
                continue;
            }
            let (_, str_off, str_size, _) = sections[strtab_idx];
            let str_off = str_off as usize;
            let str_size = str_size as usize;
            if str_off + str_size > data.len() {
                continue;
            }
            let strtab = &data[str_off..str_off + str_size];

            let sym_off = offset as usize;
            let sym_size = size as usize;
            if sym_off + sym_size > data.len() {
                continue;
            }
            let sym_data = &data[sym_off..sym_off + sym_size];
            let sym_count = sym_data.len() / 24;

            for j in 1..sym_count { // skip null symbol at index 0
                let off = j * 24;
                if off + 24 > sym_data.len() {
                    break;
                }
                let name_idx = read_u32(sym_data, off) as usize;
                let info = sym_data[off + 4];
                let shndx = read_u16(sym_data, off + 6);
                let value = read_u64(sym_data, off + 8);
                let size = read_u64(sym_data, off + 16);

                // Only include defined symbols (shndx != UND)
                if shndx == SHN_UNDEF {
                    continue;
                }

                let name = read_cstr(strtab, name_idx);
                if name.is_empty() {
                    continue;
                }

                // Strip version suffixes (e.g., "printf@@GLIBC_2.2.5" -> "printf")
                // Actually, readelf shows versions separately, the name in strtab
                // doesn't have @@ - versions are in .gnu.version section.
                // But some linker scripts may have them.

                symbols.push(DynSymbol { name, info, value, size });
            }
            break;
        }
    }

    Ok(symbols)
}

/// Get the SONAME from a shared library's .dynamic section
pub fn parse_soname(data: &[u8]) -> Option<String> {
    if data.len() < 64 || data[0..4] != ELF_MAGIC {
        return None;
    }

    let e_shoff = read_u64(data, 40) as usize;
    let e_shentsize = read_u16(data, 58) as usize;
    let e_shnum = read_u16(data, 60) as usize;

    // Find .dynamic section
    for i in 0..e_shnum {
        let off = e_shoff + i * e_shentsize;
        if off + 64 > data.len() {
            break;
        }
        let sh_type = read_u32(data, off + 4);
        if sh_type == SHT_DYNAMIC {
            let dyn_off = read_u64(data, off + 24) as usize;
            let dyn_size = read_u64(data, off + 32) as usize;
            let link = read_u32(data, off + 40) as usize;

            // Get the string table for this dynamic section
            let str_sec_off = e_shoff + link * e_shentsize;
            if str_sec_off + 64 > data.len() {
                return None;
            }
            let str_off = read_u64(data, str_sec_off + 24) as usize;
            let str_size = read_u64(data, str_sec_off + 32) as usize;
            if str_off + str_size > data.len() {
                return None;
            }
            let strtab = &data[str_off..str_off + str_size];

            // Find DT_SONAME entry
            let mut pos = dyn_off;
            while pos + 16 <= dyn_off + dyn_size && pos + 16 <= data.len() {
                let tag = read_i64(data, pos);
                let val = read_u64(data, pos + 8);
                if tag == DT_NULL {
                    break;
                }
                if tag == DT_SONAME {
                    return Some(read_cstr(strtab, val as usize));
                }
                pos += 16;
            }
        }
    }
    None
}

