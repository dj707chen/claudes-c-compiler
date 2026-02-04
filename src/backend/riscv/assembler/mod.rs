//! Native RISC-V assembler.
//!
//! Parses `.s` assembly text (as emitted by the RISC-V codegen) and produces
//! ELF `.o` object files, removing the dependency on `riscv64-linux-gnu-gcc`
//! for assembly.
//!
//! Architecture:
//! - `parser.rs`     – Tokenize + parse assembly text into `AsmStatement` items
//! - `encoder.rs`    – Encode RISC-V instructions into 32-bit machine words
//! - `compress.rs`   – RV64C compressed instruction support (32-bit → 16-bit)
//! - `elf_writer.rs` – Write ELF object files with sections, symbols, and relocations

pub mod parser;
pub mod encoder;
pub mod compress;
pub mod elf_writer;

use parser::parse_asm;
use elf_writer::ElfWriter;

/// Assemble RISC-V assembly text into an ELF object file.
///
/// This is the default assembler (used when the `gcc_assembler` feature is disabled).
#[allow(dead_code)]
pub fn assemble(asm_text: &str, output_path: &str) -> Result<(), String> {
    assemble_with_args(asm_text, output_path, &[])
}

/// Assemble RISC-V assembly text into an ELF object file, with extra args.
///
/// Supports `-mabi=` to control ELF float ABI flags (lp64/lp64f/lp64d/lp64q).
pub fn assemble_with_args(asm_text: &str, output_path: &str, extra_args: &[String]) -> Result<(), String> {
    let statements = parse_asm(asm_text)?;
    let mut writer = ElfWriter::new();

    // Parse -mabi= from extra args to set correct ELF float ABI flags
    for arg in extra_args {
        if let Some(abi) = arg.strip_prefix("-mabi=") {
            writer.set_elf_flags(elf_flags_for_abi(abi));
        }
    }

    writer.process_statements(&statements)?;
    writer.write_elf(output_path)?;
    Ok(())
}

/// Map an ABI name to ELF e_flags.
fn elf_flags_for_abi(abi: &str) -> u32 {
    const EF_RISCV_RVC: u32 = 0x1;
    const EF_RISCV_FLOAT_ABI_DOUBLE: u32 = 0x4;
    const EF_RISCV_FLOAT_ABI_SINGLE: u32 = 0x2;
    const EF_RISCV_FLOAT_ABI_QUAD: u32 = 0x6;

    let float_abi = match abi {
        "lp64" | "ilp32" => 0x0, // soft-float
        "lp64f" | "ilp32f" => EF_RISCV_FLOAT_ABI_SINGLE,
        "lp64d" | "ilp32d" => EF_RISCV_FLOAT_ABI_DOUBLE,
        "lp64q" | "ilp32q" => EF_RISCV_FLOAT_ABI_QUAD,
        _ => EF_RISCV_FLOAT_ABI_DOUBLE, // default
    };
    float_abi | EF_RISCV_RVC
}
