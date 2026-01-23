use crate::backend::Target;
use crate::frontend::preprocessor::Preprocessor;
use crate::frontend::lexer::Lexer;
use crate::frontend::parser::Parser;
use crate::frontend::sema::SemanticAnalyzer;
use crate::ir::lowering::Lowerer;
use crate::ir::mem2reg::promote_allocas;
use crate::passes::run_passes;
use crate::common::source::SourceManager;

/// Compilation mode - determines where in the pipeline to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMode {
    /// Full compilation: preprocess -> compile -> assemble -> link (default)
    Full,
    /// -S: Stop after generating assembly, output .s file
    AssemblyOnly,
    /// -c: Stop after assembling, output .o file
    ObjectOnly,
    /// -E: Stop after preprocessing, output preprocessed source to stdout
    PreprocessOnly,
}

/// A command-line define: -Dname or -Dname=value
#[derive(Debug, Clone)]
pub struct CliDefine {
    pub name: String,
    pub value: String,
}

/// The compiler driver orchestrates all compilation phases.
pub struct Driver {
    pub target: Target,
    pub output_path: String,
    pub output_path_set: bool,
    pub input_files: Vec<String>,
    pub opt_level: u32,
    pub verbose: bool,
    pub mode: CompileMode,
    pub debug_info: bool,
    pub defines: Vec<CliDefine>,
    pub include_paths: Vec<String>,
}

impl Driver {
    pub fn new() -> Self {
        Self {
            target: Target::X86_64,
            output_path: "a.out".to_string(),
            output_path_set: false,
            input_files: Vec::new(),
            opt_level: 0,
            verbose: false,
            mode: CompileMode::Full,
            debug_info: false,
            defines: Vec::new(),
            include_paths: Vec::new(),
        }
    }

    /// Add a -D define from command line.
    pub fn add_define(&mut self, arg: &str) {
        if let Some(eq_pos) = arg.find('=') {
            self.defines.push(CliDefine {
                name: arg[..eq_pos].to_string(),
                value: arg[eq_pos + 1..].to_string(),
            });
        } else {
            self.defines.push(CliDefine {
                name: arg.to_string(),
                value: "1".to_string(),
            });
        }
    }

    /// Add a -I include path from command line.
    pub fn add_include_path(&mut self, path: &str) {
        self.include_paths.push(path.to_string());
    }

    /// Determine the output path for a given input file and mode.
    fn output_for_input(&self, input_file: &str) -> String {
        if self.output_path_set {
            return self.output_path.clone();
        }
        let stem = std::path::Path::new(input_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("a");
        match self.mode {
            CompileMode::AssemblyOnly => format!("{}.s", stem),
            CompileMode::ObjectOnly => format!("{}.o", stem),
            CompileMode::PreprocessOnly => String::new(),
            CompileMode::Full => self.output_path.clone(),
        }
    }

    /// Configure the preprocessor with CLI-defined macros.
    fn configure_preprocessor(&self, preprocessor: &mut Preprocessor) {
        for def in &self.defines {
            preprocessor.define_macro(&def.name, &def.value);
        }
        for path in &self.include_paths {
            preprocessor.add_include_path(path);
        }
    }

    /// Run the compiler pipeline.
    pub fn run(&self) -> Result<(), String> {
        if self.input_files.is_empty() {
            return Err("No input files".to_string());
        }

        match self.mode {
            CompileMode::PreprocessOnly => self.run_preprocess_only(),
            CompileMode::AssemblyOnly => self.run_assembly_only(),
            CompileMode::ObjectOnly => self.run_object_only(),
            CompileMode::Full => self.run_full(),
        }
    }

    fn run_preprocess_only(&self) -> Result<(), String> {
        for input_file in &self.input_files {
            let source = std::fs::read_to_string(input_file)
                .map_err(|e| format!("Cannot read {}: {}", input_file, e))?;

            let mut preprocessor = Preprocessor::new();
            self.configure_preprocessor(&mut preprocessor);
            preprocessor.set_filename(input_file);
            let preprocessed = preprocessor.preprocess(&source);

            if self.output_path_set {
                std::fs::write(&self.output_path, &preprocessed)
                    .map_err(|e| format!("Cannot write {}: {}", self.output_path, e))?;
            } else {
                print!("{}", preprocessed);
            }
        }
        Ok(())
    }

    fn run_assembly_only(&self) -> Result<(), String> {
        for input_file in &self.input_files {
            let asm = self.compile_to_assembly(input_file)?;
            let out_path = self.output_for_input(input_file);
            std::fs::write(&out_path, &asm)
                .map_err(|e| format!("Cannot write {}: {}", out_path, e))?;
            if self.verbose {
                eprintln!("Assembly output: {}", out_path);
            }
        }
        Ok(())
    }

    fn run_object_only(&self) -> Result<(), String> {
        for input_file in &self.input_files {
            let asm = self.compile_to_assembly(input_file)?;
            let out_path = self.output_for_input(input_file);
            self.target.assemble(&asm, &out_path)?;
            if self.verbose {
                eprintln!("Object output: {}", out_path);
            }
        }
        Ok(())
    }

    fn run_full(&self) -> Result<(), String> {
        let mut object_files = Vec::new();

        for input_file in &self.input_files {
            let asm = self.compile_to_assembly(input_file)?;

            let obj_path = format!("/tmp/ccc_{}_{}.o",
                std::process::id(),
                std::path::Path::new(input_file)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("out"));
            self.target.assemble(&asm, &obj_path)?;
            object_files.push(obj_path);
        }

        let obj_refs: Vec<&str> = object_files.iter().map(|s| s.as_str()).collect();
        self.target.link(&obj_refs, &self.output_path)?;

        for obj in &object_files {
            let _ = std::fs::remove_file(obj);
        }

        if self.verbose {
            eprintln!("Output: {}", self.output_path);
        }

        Ok(())
    }

    /// Core pipeline: preprocess, lex, parse, sema, lower, optimize, codegen.
    fn compile_to_assembly(&self, input_file: &str) -> Result<String, String> {
        let source = std::fs::read_to_string(input_file)
            .map_err(|e| format!("Cannot read {}: {}", input_file, e))?;

        // Preprocess
        let mut preprocessor = Preprocessor::new();
        self.configure_preprocessor(&mut preprocessor);
        preprocessor.set_filename(input_file);
        let preprocessed = preprocessor.preprocess(&source);

        // Lex
        let mut source_manager = SourceManager::new();
        let file_id = source_manager.add_file(input_file.to_string(), preprocessed.clone());
        let mut lexer = Lexer::new(&preprocessed, file_id);
        let tokens = lexer.tokenize();

        if self.verbose {
            eprintln!("Lexed {} tokens from {}", tokens.len(), input_file);
        }

        // Parse
        let mut parser = Parser::new(tokens);
        let ast = parser.parse();

        if self.verbose {
            eprintln!("Parsed {} declarations", ast.decls.len());
        }

        // Semantic analysis
        let mut sema = SemanticAnalyzer::new();
        if let Err(errors) = sema.analyze(&ast) {
            for err in &errors {
                eprintln!("error: {}", err);
            }
            return Err(format!("{} error(s) during semantic analysis", errors.len()));
        }

        // Lower to IR
        let lowerer = Lowerer::new();
        let mut module = lowerer.lower(&ast);

        if self.verbose {
            eprintln!("Lowered to {} IR functions", module.functions.len());
        }

        // Run optimization passes
        promote_allocas(&mut module);
        run_passes(&mut module, self.opt_level);

        // Generate assembly using target-specific codegen
        let asm = self.target.generate_assembly(&module);

        if self.verbose {
            eprintln!("Generated {:?} assembly ({} bytes)", self.target, asm.len());
        }

        Ok(asm)
    }
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}
