pub mod constant_fold;
pub mod dce;

use crate::ir::ir::IrModule;

/// Run all optimization passes on the module.
pub fn run_passes(module: &mut IrModule, _opt_level: u32) {
    // TODO: implement optimization passes
    // For now, just stub them out
    let _ = module;
    // Future passes:
    // - constant_fold::run(module)
    // - dce::run(module)
    // - gvn::run(module)
    // - inline::run(module)
    // - licm::run(module)
}
