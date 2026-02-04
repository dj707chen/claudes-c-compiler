//! Shared ELF types, constants, and utilities used by all assembler and linker backends.
//!
//! This module eliminates duplication of ELF infrastructure across x86, i686, ARM,
//! and RISC-V backends. It provides:
//!
//! - ELF format constants (section types, flags, symbol bindings, etc.)
//! - `StringTable` for building ELF string tables (.strtab, .shstrtab, .dynstr)
//! - Binary read/write helpers for little-endian ELF fields
//! - `write_shdr64` / `write_shdr32` for section header emission
//! - `parse_archive_members` for reading `.a` static archives
//! - `parse_linker_script` for handling linker script GROUP directives

use std::collections::HashMap;

// ── ELF identification ───────────────────────────────────────────────────────

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

// ELF class
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;

// Data encoding
pub const ELFDATA2LSB: u8 = 1;

// Version
pub const EV_CURRENT: u8 = 1;

// OS/ABI
pub const ELFOSABI_NONE: u8 = 0;

// ── ELF object types ─────────────────────────────────────────────────────────

pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

// ── Machine types ────────────────────────────────────────────────────────────

pub const EM_386: u16 = 3;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;
pub const EM_RISCV: u16 = 243;

// ── Section header types ─────────────────────────────────────────────────────

pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_HASH: u32 = 5;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;
pub const SHT_INIT_ARRAY: u32 = 14;
pub const SHT_FINI_ARRAY: u32 = 15;
pub const SHT_PREINIT_ARRAY: u32 = 16;
pub const SHT_GROUP: u32 = 17;
pub const SHT_GNU_HASH: u32 = 0x6fff_fff6;
pub const SHT_GNU_VERSYM: u32 = 0x6fff_ffff;
pub const SHT_GNU_VERNEED: u32 = 0x6fff_fffe;
pub const SHT_GNU_VERDEF: u32 = 0x6fff_fffd;

// ── Section header flags ─────────────────────────────────────────────────────

pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;
pub const SHF_MERGE: u64 = 0x10;
pub const SHF_STRINGS: u64 = 0x20;
pub const SHF_INFO_LINK: u64 = 0x40;
pub const SHF_GROUP: u64 = 0x200;
pub const SHF_TLS: u64 = 0x400;
pub const SHF_EXCLUDE: u64 = 0x8000_0000;

// ── Symbol binding ───────────────────────────────────────────────────────────

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;

// ── Symbol types ─────────────────────────────────────────────────────────────

pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;
pub const STT_COMMON: u8 = 5;
pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;

// ── Symbol visibility ────────────────────────────────────────────────────────

pub const STV_DEFAULT: u8 = 0;
pub const STV_INTERNAL: u8 = 1;
pub const STV_HIDDEN: u8 = 2;
pub const STV_PROTECTED: u8 = 3;

// ── Special section indices ──────────────────────────────────────────────────

pub const SHN_UNDEF: u16 = 0;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;

// ── Program header types ─────────────────────────────────────────────────────

pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;

// ── Program header flags ─────────────────────────────────────────────────────

pub const PF_X: u32 = 0x1;
pub const PF_W: u32 = 0x2;
pub const PF_R: u32 = 0x4;

// ── Dynamic section tags ─────────────────────────────────────────────────────

pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTGOT: i64 = 3;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_JMPREL: i64 = 23;
pub const DT_PLTREL: i64 = 20;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;

// ── ELF sizes ────────────────────────────────────────────────────────────────

/// Size of ELF64 header in bytes.
pub const ELF64_EHDR_SIZE: usize = 64;
/// Size of ELF32 header in bytes.
pub const ELF32_EHDR_SIZE: usize = 52;
/// Size of ELF64 section header in bytes.
pub const ELF64_SHDR_SIZE: usize = 64;
/// Size of ELF32 section header in bytes.
pub const ELF32_SHDR_SIZE: usize = 40;
/// Size of ELF64 symbol table entry in bytes.
pub const ELF64_SYM_SIZE: usize = 24;
/// Size of ELF32 symbol table entry in bytes.
pub const ELF32_SYM_SIZE: usize = 16;
/// Size of ELF64 RELA relocation entry in bytes.
pub const ELF64_RELA_SIZE: usize = 24;
/// Size of ELF32 REL relocation entry in bytes.
pub const ELF32_REL_SIZE: usize = 8;
/// Size of ELF64 program header in bytes.
pub const ELF64_PHDR_SIZE: usize = 56;
/// Size of ELF32 program header in bytes.
pub const ELF32_PHDR_SIZE: usize = 32;

// ── String table ─────────────────────────────────────────────────────────────

/// ELF string table builder. Used for .strtab, .shstrtab, and .dynstr sections.
///
/// Strings are stored as null-terminated entries. The table always starts with
/// a null byte (index 0 = empty string), matching ELF convention.
pub struct StringTable {
    data: Vec<u8>,
    offsets: HashMap<String, u32>,
}

impl StringTable {
    /// Create a new string table with the initial null byte.
    pub fn new() -> Self {
        Self {
            data: vec![0],
            offsets: HashMap::new(),
        }
    }

    /// Add a string to the table and return its offset.
    /// Returns 0 for empty strings. Deduplicates repeated insertions.
    pub fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        if let Some(&offset) = self.offsets.get(s) {
            return offset;
        }
        let offset = self.data.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
        self.offsets.insert(s.to_string(), offset);
        offset
    }

    /// Look up the offset of a previously-added string. Returns 0 if not found.
    pub fn offset_of(&self, s: &str) -> u32 {
        self.offsets.get(s).copied().unwrap_or(0)
    }

    /// Return the raw table bytes (including the leading null byte).
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Return the size of the table in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

// ── Binary read helpers (little-endian) ──────────────────────────────────────

/// Read a little-endian u16 from `data` at `offset`.
#[inline]
pub fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Read a little-endian u32 from `data` at `offset`.
#[inline]
pub fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ])
}

/// Read a little-endian u64 from `data` at `offset`.
#[inline]
pub fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
    ])
}

/// Read a little-endian i32 from `data` at `offset`.
#[inline]
pub fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ])
}

/// Read a little-endian i64 from `data` at `offset`.
#[inline]
pub fn read_i64(data: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
    ])
}

/// Read a null-terminated string from a byte slice starting at `offset`.
pub fn read_cstr(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    let end = data[offset..].iter().position(|&b| b == 0).unwrap_or(data.len() - offset);
    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
}

// ── Binary write helpers (little-endian, in-place) ───────────────────────────

/// Write a little-endian u16 into `buf` at `offset`. No-op if out of bounds.
#[inline]
pub fn w16(buf: &mut [u8], off: usize, val: u16) {
    if off + 2 <= buf.len() {
        buf[off..off + 2].copy_from_slice(&val.to_le_bytes());
    }
}

/// Write a little-endian u32 into `buf` at `offset`. No-op if out of bounds.
#[inline]
pub fn w32(buf: &mut [u8], off: usize, val: u32) {
    if off + 4 <= buf.len() {
        buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
    }
}

/// Write a little-endian u64 into `buf` at `offset`. No-op if out of bounds.
#[inline]
pub fn w64(buf: &mut [u8], off: usize, val: u64) {
    if off + 8 <= buf.len() {
        buf[off..off + 8].copy_from_slice(&val.to_le_bytes());
    }
}

/// Copy `data` into `buf` starting at `off`. No-op if out of bounds.
#[inline]
pub fn write_bytes(buf: &mut [u8], off: usize, data: &[u8]) {
    let end = off + data.len();
    if end <= buf.len() {
        buf[off..end].copy_from_slice(data);
    }
}

// ── Section header writing ───────────────────────────────────────────────────

/// Append an ELF64 section header to `buf`.
pub fn write_shdr64(
    buf: &mut Vec<u8>,
    sh_name: u32, sh_type: u32, sh_flags: u64,
    sh_addr: u64, sh_offset: u64, sh_size: u64,
    sh_link: u32, sh_info: u32, sh_addralign: u64, sh_entsize: u64,
) {
    buf.extend_from_slice(&sh_name.to_le_bytes());
    buf.extend_from_slice(&sh_type.to_le_bytes());
    buf.extend_from_slice(&sh_flags.to_le_bytes());
    buf.extend_from_slice(&sh_addr.to_le_bytes());
    buf.extend_from_slice(&sh_offset.to_le_bytes());
    buf.extend_from_slice(&sh_size.to_le_bytes());
    buf.extend_from_slice(&sh_link.to_le_bytes());
    buf.extend_from_slice(&sh_info.to_le_bytes());
    buf.extend_from_slice(&sh_addralign.to_le_bytes());
    buf.extend_from_slice(&sh_entsize.to_le_bytes());
}

/// Append an ELF32 section header to `buf`.
pub fn write_shdr32(
    buf: &mut Vec<u8>,
    sh_name: u32, sh_type: u32, sh_flags: u32,
    sh_addr: u32, sh_offset: u32, sh_size: u32,
    sh_link: u32, sh_info: u32, sh_addralign: u32, sh_entsize: u32,
) {
    buf.extend_from_slice(&sh_name.to_le_bytes());
    buf.extend_from_slice(&sh_type.to_le_bytes());
    buf.extend_from_slice(&sh_flags.to_le_bytes());
    buf.extend_from_slice(&sh_addr.to_le_bytes());
    buf.extend_from_slice(&sh_offset.to_le_bytes());
    buf.extend_from_slice(&sh_size.to_le_bytes());
    buf.extend_from_slice(&sh_link.to_le_bytes());
    buf.extend_from_slice(&sh_info.to_le_bytes());
    buf.extend_from_slice(&sh_addralign.to_le_bytes());
    buf.extend_from_slice(&sh_entsize.to_le_bytes());
}

/// Write an ELF64 program header to `buf` at offset `off`.
pub fn write_phdr64(
    buf: &mut [u8], off: usize,
    p_type: u32, p_flags: u32, p_offset: u64,
    p_vaddr: u64, p_paddr: u64, p_filesz: u64, p_memsz: u64, p_align: u64,
) {
    w32(buf, off, p_type);
    w32(buf, off + 4, p_flags);
    w64(buf, off + 8, p_offset);
    w64(buf, off + 16, p_vaddr);
    w64(buf, off + 24, p_paddr);
    w64(buf, off + 32, p_filesz);
    w64(buf, off + 40, p_memsz);
    w64(buf, off + 48, p_align);
}

/// Write an ELF64 symbol table entry to `buf`.
pub fn write_sym64(
    buf: &mut Vec<u8>,
    st_name: u32, st_info: u8, st_other: u8, st_shndx: u16,
    st_value: u64, st_size: u64,
) {
    buf.extend_from_slice(&st_name.to_le_bytes());
    buf.push(st_info);
    buf.push(st_other);
    buf.extend_from_slice(&st_shndx.to_le_bytes());
    buf.extend_from_slice(&st_value.to_le_bytes());
    buf.extend_from_slice(&st_size.to_le_bytes());
}

/// Write an ELF32 symbol table entry to `buf`.
pub fn write_sym32(
    buf: &mut Vec<u8>,
    st_name: u32, st_value: u32, st_size: u32,
    st_info: u8, st_other: u8, st_shndx: u16,
) {
    buf.extend_from_slice(&st_name.to_le_bytes());
    buf.extend_from_slice(&st_value.to_le_bytes());
    buf.extend_from_slice(&st_size.to_le_bytes());
    buf.push(st_info);
    buf.push(st_other);
    buf.extend_from_slice(&st_shndx.to_le_bytes());
}

/// Write an ELF64 RELA relocation entry to `buf`.
pub fn write_rela64(buf: &mut Vec<u8>, r_offset: u64, r_sym: u32, r_type: u32, r_addend: i64) {
    buf.extend_from_slice(&r_offset.to_le_bytes());
    let r_info: u64 = ((r_sym as u64) << 32) | (r_type as u64);
    buf.extend_from_slice(&r_info.to_le_bytes());
    buf.extend_from_slice(&r_addend.to_le_bytes());
}

/// Write an ELF32 REL relocation entry to `buf`.
pub fn write_rel32(buf: &mut Vec<u8>, r_offset: u32, r_sym: u32, r_type: u8) {
    buf.extend_from_slice(&r_offset.to_le_bytes());
    let r_info: u32 = (r_sym << 8) | (r_type as u32);
    buf.extend_from_slice(&r_info.to_le_bytes());
}

// ── Archive (.a) parsing ─────────────────────────────────────────────────────

/// Parse a GNU-format static archive (.a file), returning member entries as
/// `(name, data_offset, data_size)` tuples. The offsets are into the original
/// `data` slice, enabling zero-copy access.
///
/// Handles extended name tables (`//`), symbol tables (`/`, `/SYM64/`), and
/// 2-byte alignment padding between members.
pub fn parse_archive_members(data: &[u8]) -> Result<Vec<(String, usize, usize)>, String> {
    if data.len() < 8 || &data[0..8] != b"!<arch>\n" {
        return Err("not a valid archive file".to_string());
    }

    let mut members = Vec::new();
    let mut pos = 8;
    let mut extended_names: Option<&[u8]> = None;

    while pos + 60 <= data.len() {
        let name_raw = &data[pos..pos + 16];
        let size_str = std::str::from_utf8(&data[pos + 48..pos + 58])
            .unwrap_or("")
            .trim();
        let magic = &data[pos + 58..pos + 60];
        if magic != b"`\n" {
            break;
        }

        let size: usize = size_str.parse().unwrap_or(0);
        let data_start = pos + 60;
        let name_str = std::str::from_utf8(name_raw).unwrap_or("").trim_end();

        if name_str == "/" || name_str == "/SYM64/" {
            // Symbol table — skip
        } else if name_str == "//" {
            // Extended name table
            extended_names = Some(&data[data_start..(data_start + size).min(data.len())]);
        } else {
            let member_name = if name_str.starts_with('/') {
                // Extended name: /offset into extended names table
                if let Some(ext) = extended_names {
                    let name_off: usize = name_str[1..].trim_end_matches('/').parse().unwrap_or(0);
                    if name_off < ext.len() {
                        let end = ext[name_off..]
                            .iter()
                            .position(|&b| b == b'/' || b == b'\n' || b == 0)
                            .unwrap_or(ext.len() - name_off);
                        String::from_utf8_lossy(&ext[name_off..name_off + end]).to_string()
                    } else {
                        name_str.to_string()
                    }
                } else {
                    name_str.to_string()
                }
            } else {
                name_str.trim_end_matches('/').to_string()
            };

            if data_start + size <= data.len() {
                members.push((member_name, data_start, size));
            }
        }

        // Align to 2-byte boundary
        pos = data_start + size;
        if pos % 2 != 0 {
            pos += 1;
        }
    }

    Ok(members)
}

// ── Linker script parsing ────────────────────────────────────────────────────

/// Parse a linker script looking for `GROUP ( ... )` directives.
/// Returns the list of library paths referenced, or `None` if no GROUP found.
///
/// Filters out AS_NEEDED entries (these are optional shared libraries that
/// the static linker does not need to handle).
pub fn parse_linker_script(content: &str) -> Option<Vec<String>> {
    let group_start = content.find("GROUP")?;
    let paren_start = content[group_start..].find('(')?;
    let paren_end = content[group_start..].find(')')?;
    let inside = &content[group_start + paren_start + 1..group_start + paren_end];

    let mut paths = Vec::new();
    let mut in_as_needed = false;
    for token in inside.split_whitespace() {
        match token {
            "AS_NEEDED" => { in_as_needed = true; continue; }
            "(" => continue,
            ")" => { in_as_needed = false; continue; }
            _ => {}
        }
        if token.starts_with('/') || token.ends_with(".so") || token.ends_with(".a") ||
           token.contains(".so.") {
            if !in_as_needed {
                paths.push(token.to_string());
            }
        }
    }

    if paths.is_empty() { None } else { Some(paths) }
}

// ── Section name mapping ─────────────────────────────────────────────────────

// ── Assembler helpers ────────────────────────────────────────────────────────

/// Map a symbol's section name to its index in the section header table.
///
/// Handles special pseudo-sections used during assembly:
/// - `*COM*` → `SHN_COMMON` (0xFFF2) for COMMON symbols
/// - `*UND*` or empty → `SHN_UNDEF` (0) for undefined symbols
/// - Otherwise, looks up the section in the content section list (1-based index)
pub fn section_index(section_name: &str, content_sections: &[String]) -> u16 {
    if section_name == "*COM*" {
        SHN_COMMON
    } else if section_name == "*UND*" || section_name.is_empty() {
        SHN_UNDEF
    } else {
        content_sections.iter().position(|s| s == section_name)
            .map(|i| (i + 1) as u16)
            .unwrap_or(SHN_UNDEF)
    }
}

/// Return default ELF section flags based on section name conventions.
///
/// These are the standard mappings: `.text.*` → alloc+exec, `.data.*` → alloc+write,
/// `.rodata.*` → alloc, `.bss.*` → alloc+write, `.tdata`/`.tbss` → alloc+write+TLS, etc.
pub fn default_section_flags(name: &str) -> u64 {
    if name == ".text" || name.starts_with(".text.") {
        SHF_ALLOC | SHF_EXECINSTR
    } else if name == ".data" || name.starts_with(".data.") {
        SHF_ALLOC | SHF_WRITE
    } else if name == ".bss" || name.starts_with(".bss.") {
        SHF_ALLOC | SHF_WRITE
    } else if name == ".rodata" || name.starts_with(".rodata.") {
        SHF_ALLOC
    } else if name == ".note.GNU-stack" {
        0 // Non-executable stack marker, no flags
    } else if name.starts_with(".note") {
        SHF_ALLOC
    } else if name.starts_with(".tdata") {
        SHF_ALLOC | SHF_WRITE | SHF_TLS
    } else if name.starts_with(".tbss") {
        SHF_ALLOC | SHF_WRITE | SHF_TLS
    } else if name.starts_with(".init") || name.starts_with(".fini") {
        SHF_ALLOC | SHF_EXECINSTR
    } else {
        0
    }
}
