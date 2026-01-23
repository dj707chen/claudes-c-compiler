pub mod x86;
pub mod arm;
pub mod riscv;

/// Target architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    X86_64,
    Aarch64,
    Riscv64,
}
