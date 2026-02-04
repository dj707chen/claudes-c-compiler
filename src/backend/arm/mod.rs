pub(crate) mod codegen;
#[cfg_attr(feature = "gcc_assembler", allow(dead_code))]
pub(crate) mod assembler;
pub(crate) mod linker;

pub(crate) use codegen::emit::ArmCodegen;
