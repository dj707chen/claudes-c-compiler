//! Native x86-64 assembler: parses AT&T syntax assembly and produces ELF .o files.
//!
//! This module replaces the need for an external `gcc -c` call when assembling
//! compiler-generated x86-64 assembly. It handles the subset of AT&T syntax
//! that our codegen actually emits.
//!
//! Architecture:
//! - `parser.rs`: Tokenize and parse AT&T syntax assembly text into `AsmItem`s
//! - `encoder.rs`: Encode x86-64 instructions into machine code bytes
//! - `elf.rs`: Write ELF relocatable object files (.o)

pub(crate) mod parser;
pub(crate) mod encoder;
pub(crate) mod elf;

/// Assemble AT&T syntax x86-64 assembly text into an ELF .o file.
/// Returns the ELF object file contents as bytes.
pub fn assemble(asm_text: &str) -> Result<Vec<u8>, String> {
    let items = parser::parse(asm_text)?;
    let obj = elf::ObjectBuilder::new();
    obj.build(&items)
}
