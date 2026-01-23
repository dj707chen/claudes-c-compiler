/// Minimal preprocessor that strips preprocessor directives.
///
/// For the initial scaffold, we handle:
/// - #include directives (by recording them but stripping from output)
/// - #define / #undef (stripped, not yet processed)
/// - #if / #ifdef / #ifndef / #else / #elif / #endif (stripped)
/// - #pragma (stripped)
///
/// The preprocessor produces cleaned source code ready for lexing.
pub struct Preprocessor {
    includes: Vec<String>,
}

impl Preprocessor {
    pub fn new() -> Self {
        Self {
            includes: Vec::new(),
        }
    }

    /// Process source code, stripping preprocessor directives.
    /// Returns the preprocessed source.
    pub fn preprocess(&mut self, source: &str) -> String {
        let mut output = String::with_capacity(source.len());
        let mut ifdef_depth: i32 = 0;
        let skipping = false; // TODO: implement conditional compilation

        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                let directive = self.parse_directive(trimmed);
                match directive {
                    Directive::Include(path) => {
                        self.includes.push(path);
                        // Insert a newline to keep line numbers aligned
                        output.push('\n');
                    }
                    Directive::Define { .. } => {
                        // TODO: implement macro definitions
                        output.push('\n');
                    }
                    Directive::Undef(_) => {
                        output.push('\n');
                    }
                    Directive::If | Directive::Ifdef | Directive::Ifndef => {
                        ifdef_depth += 1;
                        output.push('\n');
                    }
                    Directive::Elif | Directive::Else => {
                        output.push('\n');
                    }
                    Directive::Endif => {
                        ifdef_depth -= 1;
                        if ifdef_depth < 0 {
                            ifdef_depth = 0;
                        }
                        output.push('\n');
                    }
                    Directive::Pragma => {
                        output.push('\n');
                    }
                    Directive::Error(_) => {
                        // TODO: emit error
                        output.push('\n');
                    }
                    Directive::Unknown => {
                        output.push('\n');
                    }
                }
            } else {
                if !skipping {
                    output.push_str(line);
                }
                output.push('\n');
            }
        }

        let _ = ifdef_depth;
        output
    }

    /// Get the list of includes encountered during preprocessing.
    pub fn includes(&self) -> &[String] {
        &self.includes
    }

    fn parse_directive(&self, line: &str) -> Directive {
        let line = line.trim_start_matches('#').trim();

        if line.starts_with("include") {
            let rest = line.strip_prefix("include").unwrap().trim();
            let path = if rest.starts_with('<') {
                rest.trim_start_matches('<').trim_end_matches('>').to_string()
            } else if rest.starts_with('"') {
                rest.trim_matches('"').to_string()
            } else {
                rest.to_string()
            };
            Directive::Include(path)
        } else if line.starts_with("define") {
            Directive::Define { name: String::new(), value: None }
        } else if line.starts_with("undef") {
            let name = line.strip_prefix("undef").unwrap().trim().to_string();
            Directive::Undef(name)
        } else if line.starts_with("ifdef") || line.starts_with("ifndef") {
            if line.starts_with("ifndef") {
                Directive::Ifndef
            } else {
                Directive::Ifdef
            }
        } else if line.starts_with("if") {
            Directive::If
        } else if line.starts_with("elif") {
            Directive::Elif
        } else if line.starts_with("else") {
            Directive::Else
        } else if line.starts_with("endif") {
            Directive::Endif
        } else if line.starts_with("pragma") {
            Directive::Pragma
        } else if line.starts_with("error") {
            let msg = line.strip_prefix("error").unwrap().trim().to_string();
            Directive::Error(msg)
        } else {
            Directive::Unknown
        }
    }
}

#[derive(Debug)]
enum Directive {
    Include(String),
    Define { name: String, value: Option<String> },
    Undef(String),
    If,
    Ifdef,
    Ifndef,
    Elif,
    Else,
    Endif,
    Pragma,
    Error(String),
    Unknown,
}

impl Default for Preprocessor {
    fn default() -> Self {
        Self::new()
    }
}
