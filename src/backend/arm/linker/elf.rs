//! ELF64 parsing for the AArch64 linker.
//!
//! Reads ELF relocatable object files (.o), static archives (.a), and
//! shared libraries (.so), extracting sections, symbols, and relocations.
//!
//! ELF constants, read/write helpers, archive parsing, and linker script
//! parsing are imported from the shared `crate::backend::elf` module.

// Re-export shared ELF constants so existing callers (mod.rs, reloc.rs)
// continue to work via `use super::elf::*`.
pub use crate::backend::elf::{
    ELF_MAGIC, ELFCLASS64, ELFDATA2LSB, ET_REL, ET_EXEC, ET_DYN, EM_AARCH64,
    SHT_NULL, SHT_PROGBITS, SHT_SYMTAB, SHT_STRTAB, SHT_RELA,
    SHT_NOBITS, SHT_REL, SHT_GROUP,
    SHF_WRITE, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_EXCLUDE,
    STB_LOCAL, STB_GLOBAL, STB_WEAK,
    STT_OBJECT, STT_FUNC, STT_SECTION, STT_FILE, STT_TLS, STT_GNU_IFUNC,
    SHN_UNDEF, SHN_ABS, SHN_COMMON,
    PT_LOAD, PT_TLS, PT_GNU_STACK,
    PF_X, PF_W, PF_R,
    read_u16, read_u32, read_u64, read_i64, read_cstr,
    w16, w32, w64, write_bytes,
    parse_archive_members, parse_linker_script,
};

// ── AArch64 relocation types ───────────────────────────────────────────

pub const R_AARCH64_NONE: u32 = 0;
pub const R_AARCH64_ABS64: u32 = 257;     // S + A
pub const R_AARCH64_ABS32: u32 = 258;     // S + A (32-bit)
pub const R_AARCH64_ABS16: u32 = 259;     // S + A (16-bit)
pub const R_AARCH64_PREL64: u32 = 260;    // S + A - P
pub const R_AARCH64_PREL32: u32 = 261;    // S + A - P
pub const R_AARCH64_PREL16: u32 = 262;    // S + A - P
pub const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;  // Page(S+A) - Page(P)
pub const R_AARCH64_ADR_PREL_LO21: u32 = 274;     // S + A - P
pub const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;   // (S + A) & 0xFFF
pub const R_AARCH64_LDST8_ABS_LO12_NC: u32 = 278;
pub const R_AARCH64_LDST16_ABS_LO12_NC: u32 = 284;
pub const R_AARCH64_LDST32_ABS_LO12_NC: u32 = 285;
pub const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;
pub const R_AARCH64_LDST128_ABS_LO12_NC: u32 = 299;
pub const R_AARCH64_JUMP26: u32 = 282;    // S + A - P (26-bit B)
pub const R_AARCH64_CALL26: u32 = 283;    // S + A - P (26-bit BL)
pub const R_AARCH64_MOVW_UABS_G0_NC: u32 = 264;
pub const R_AARCH64_MOVW_UABS_G1_NC: u32 = 265;
pub const R_AARCH64_MOVW_UABS_G2_NC: u32 = 266;
pub const R_AARCH64_MOVW_UABS_G3: u32 = 267;
pub const R_AARCH64_MOVW_UABS_G0: u32 = 263;
pub const R_AARCH64_ADR_GOT_PAGE: u32 = 311;
pub const R_AARCH64_LD64_GOT_LO12_NC: u32 = 312;
pub const R_AARCH64_CONDBR19: u32 = 280;
pub const R_AARCH64_TSTBR14: u32 = 279;

// ── Data structures ────────────────────────────────────────────────────

/// Parsed ELF section header
#[derive(Debug, Clone)]
pub struct SectionHeader {
    pub name: String,
    pub sh_type: u32,
    pub flags: u64,
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
    pub name: String,
    pub info: u8,
    pub other: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

impl Symbol {
    pub fn binding(&self) -> u8 { self.info >> 4 }
    pub fn sym_type(&self) -> u8 { self.info & 0xf }
    pub fn is_undefined(&self) -> bool { self.shndx == SHN_UNDEF }
    pub fn is_global(&self) -> bool { self.binding() == STB_GLOBAL }
    pub fn is_weak(&self) -> bool { self.binding() == STB_WEAK }
    pub fn is_local(&self) -> bool { self.binding() == STB_LOCAL }
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

// ── ELF parsing ────────────────────────────────────────────────────────

/// Parse an ELF64 relocatable object file (.o) for AArch64
pub fn parse_object(data: &[u8], source_name: &str) -> Result<ElfObject, String> {
    if data.len() < 64 {
        return Err(format!("{}: file too small for ELF header", source_name));
    }
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
    if e_machine != EM_AARCH64 {
        return Err(format!("{}: not AArch64 (machine={})", source_name, e_machine));
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
            name: String::new(),
            sh_type: read_u32(data, off + 4),
            flags: read_u64(data, off + 8),
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
            let name_idxs: Vec<u32> = (0..e_shnum).map(|i| {
                read_u32(data, e_shoff + i * e_shentsize)
            }).collect();
            for (i, sec) in sections.iter_mut().enumerate() {
                sec.name = read_cstr(strtab_data, name_idxs[i] as usize);
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

    // Find symbol table
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
            let sym_count = sym_data.len() / 24;
            for j in 0..sym_count {
                let off = j * 24;
                if off + 24 > sym_data.len() { break; }
                let name_idx = read_u32(sym_data, off);
                symbols.push(Symbol {
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

    // Parse relocations
    let mut relocations = vec![Vec::new(); e_shnum];
    for i in 0..sections.len() {
        if sections[i].sh_type == SHT_RELA {
            let target_sec = sections[i].info as usize;
            let rela_data = &section_data[i];
            let rela_count = rela_data.len() / 24;
            let mut relas = Vec::with_capacity(rela_count);
            for j in 0..rela_count {
                let off = j * 24;
                if off + 24 > rela_data.len() { break; }
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
