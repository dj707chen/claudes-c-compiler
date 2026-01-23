use crate::frontend::parser::ast::*;

/// Minimal semantic analyzer.
/// For the scaffold, this just passes through the AST without real type checking.
/// TODO: Implement full type checking, scope analysis, implicit conversions.
pub struct SemanticAnalyzer {
    // TODO: symbol table, type environment
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self {}
    }

    /// Analyze a translation unit. Currently a no-op pass-through.
    pub fn analyze(&mut self, tu: &TranslationUnit) -> Result<(), Vec<String>> {
        // TODO: implement proper semantic analysis
        // For now, just verify basic structure
        for decl in &tu.decls {
            match decl {
                ExternalDecl::FunctionDef(func) => {
                    self.check_function(func)?;
                }
                ExternalDecl::Declaration(_) => {
                    // TODO: check declarations
                }
            }
        }
        Ok(())
    }

    fn check_function(&mut self, _func: &FunctionDef) -> Result<(), Vec<String>> {
        // TODO: check function body, return types, etc.
        Ok(())
    }
}

impl Default for SemanticAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
