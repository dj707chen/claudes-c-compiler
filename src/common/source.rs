/// A byte-offset span in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub file_id: u32,
}

impl Span {
    pub fn new(start: u32, end: u32, file_id: u32) -> Self {
        Self { start, end, file_id }
    }

    pub fn dummy() -> Self {
        Self { start: 0, end: 0, file_id: 0 }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            file_id: self.file_id,
        }
    }
}

/// A human-readable source location.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// Entry in the line map: maps a byte offset in the preprocessed output
/// to an original filename and line number. Uses a filename index into the
/// deduplicated `line_map_filenames` table to avoid per-entry String allocation.
#[derive(Debug, Clone, Copy)]
struct LineMapEntry {
    /// Byte offset in preprocessed output where this mapping starts.
    pp_offset: u32,
    /// Index into SourceManager::line_map_filenames.
    filename_idx: u16,
    /// Original line number (1-based) at pp_offset.
    orig_line: u32,
}

/// Manages source files and provides span-to-location resolution.
///
/// Supports two modes:
/// 1. Simple mode: a single file registered via `add_file()`, spans resolved
///    directly via byte-offset-to-line binary search.
/// 2. Line-map mode: preprocessed output with embedded `# linenum "filename"`
///    markers. The line map is built by `build_line_map()` and used to resolve
///    spans back to original source files and line numbers.
#[derive(Debug, Default)]
pub struct SourceManager {
    files: Vec<SourceFile>,
    /// Line map entries sorted by pp_offset. When non-empty, resolve_span uses
    /// this instead of per-file line_offsets.
    line_map: Vec<LineMapEntry>,
    /// Deduplicated filename strings referenced by LineMapEntry::filename_idx.
    /// Avoids allocating the same filename string for every line marker.
    line_map_filenames: Vec<String>,
}

#[derive(Debug)]
struct SourceFile {
    name: String,
    content: String,
    line_offsets: Vec<u32>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            line_map: Vec::new(),
            line_map_filenames: Vec::new(),
        }
    }

    pub fn add_file(&mut self, name: String, content: String) -> u32 {
        let line_offsets = compute_line_offsets(&content);
        let id = self.files.len() as u32;
        self.files.push(SourceFile { name, content, line_offsets });
        id
    }

    pub fn get_content(&self, file_id: u32) -> &str {
        &self.files[file_id as usize].content
    }

    pub fn get_filename(&self, file_id: u32) -> &str {
        &self.files[file_id as usize].name
    }

    /// Build a line map from GCC-style line markers in preprocessed output.
    ///
    /// Scans the stored file content (files[0]) for lines matching
    /// `# <number> "<filename>"`. These markers are emitted by the preprocessor
    /// at `#include` boundaries and indicate that subsequent lines originate
    /// from the named file starting at the given line number.
    ///
    /// Reuses the line offsets already computed by `add_file()` for column
    /// calculation, avoiding a redundant scan of the preprocessed output.
    ///
    /// Must be called after `add_file()` has stored the preprocessed content.
    pub fn build_line_map(&mut self) {
        if self.files.is_empty() {
            return;
        }

        // Line offsets for column calculation are reused directly from
        // files[0].line_offsets (computed once during add_file), avoiding
        // both a redundant O(n) scan and a Vec clone.

        let bytes = self.files[0].content.as_bytes();
        let len = bytes.len();

        // Track the last filename (byte range) and its index to avoid redundant
        // lookups. Consecutive line markers usually reference the same file.
        let mut last_fname_start: usize = 0;
        let mut last_fname_end: usize = 0;
        let mut last_fname_idx: u16 = 0;
        use crate::common::fx_hash::FxHashMap;
        let mut fname_map: FxHashMap<&[u8], u16> = FxHashMap::default();

        let mut i = 0;
        while i < len {
            // Find end of this line using fast newline search
            let line_end = if let Some(rel) = memchr_newline(&bytes[i..]) {
                i + rel
            } else {
                len
            };

            // Quick check: line markers start with '#' (possibly after whitespace).
            // Skip lines that don't start with '#' for fast rejection.
            let mut j = i;
            while j < line_end && bytes[j] == b' ' {
                j += 1;
            }

            if j < line_end && bytes[j] == b'#' {
                j += 1;
                // Skip whitespace after #
                while j < line_end && bytes[j] == b' ' {
                    j += 1;
                }
                // Parse line number directly from bytes (avoids from_utf8 + parse)
                let num_start = j;
                while j < line_end && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > num_start {
                    if let Some(line_num) = parse_u32_from_digits(&bytes[num_start..j]) {
                        // Skip whitespace
                        while j < line_end && bytes[j] == b' ' {
                            j += 1;
                        }
                        // Parse "filename"
                        if j < line_end && bytes[j] == b'"' {
                            j += 1;
                            let fname_start = j;
                            while j < line_end && bytes[j] != b'"' {
                                j += 1;
                            }
                            let fname_bytes = &bytes[fname_start..j];

                            // Deduplicate filenames: check last-used cache first
                            // (consecutive markers usually reference the same file),
                            // then fall back to the hash map for non-consecutive repeats.
                            let filename_idx =
                                if last_fname_end > last_fname_start
                                    && fname_bytes == &bytes[last_fname_start..last_fname_end]
                                {
                                    // Same filename as previous marker (common case)
                                    last_fname_idx
                                } else if let Some(&idx) = fname_map.get(fname_bytes) {
                                    // Previously seen filename
                                    idx
                                } else {
                                    // New unique filename - allocate once
                                    let s = std::str::from_utf8(fname_bytes)
                                        .unwrap_or("<unknown>")
                                        .to_string();
                                    let idx = self.line_map_filenames.len() as u16;
                                    self.line_map_filenames.push(s);
                                    fname_map.insert(fname_bytes, idx);
                                    idx
                                };
                            last_fname_start = fname_start;
                            last_fname_end = j;
                            last_fname_idx = filename_idx;

                            // The next line (after this marker) maps to filename:line_num.
                            // Record the byte offset of the line after the marker.
                            let next_line_offset = if line_end < len {
                                line_end + 1 // skip the '\n'
                            } else {
                                line_end
                            };

                            self.line_map.push(LineMapEntry {
                                pp_offset: next_line_offset as u32,
                                filename_idx,
                                orig_line: line_num,
                            });
                        }
                    }
                }
            }

            // Advance past the newline
            i = if line_end < len { line_end + 1 } else { len };
        }
    }

    /// Resolve a span to a human-readable source location.
    ///
    /// When a line map is available (preprocessor emitted line markers),
    /// resolves through the line map to the original file and line number.
    /// Otherwise falls back to direct file-based resolution.
    pub fn resolve_span(&self, span: Span) -> SourceLocation {
        if !self.line_map.is_empty() {
            return self.resolve_via_line_map(span);
        }

        // Fallback: direct file-based resolution
        if (span.file_id as usize) >= self.files.len() {
            return SourceLocation {
                file: "<unknown>".to_string(),
                line: 0,
                column: 0,
            };
        }
        let file = &self.files[span.file_id as usize];
        let line = match file.line_offsets.binary_search(&span.start) {
            Ok(i) => i as u32,
            Err(i) => if i > 0 { (i - 1) as u32 } else { 0 },
        };
        let col = span.start.saturating_sub(file.line_offsets[line as usize]);
        SourceLocation {
            file: file.name.clone(),
            line: line + 1,
            column: col + 1,
        }
    }

    /// Resolve a span using the line map built from preprocessor line markers.
    /// Assumes files[0] contains the preprocessed output (set by the driver via add_file).
    fn resolve_via_line_map(&self, span: Span) -> SourceLocation {
        let offset = span.start;

        // Find the line map entry that covers this offset.
        // Binary search for the last entry with pp_offset <= offset.
        let idx = match self.line_map.binary_search_by_key(&offset, |e| e.pp_offset) {
            Ok(i) => i,
            Err(i) => if i > 0 { i - 1 } else { 0 },
        };

        let entry = &self.line_map[idx];

        // Count how many newlines are between entry.pp_offset and offset
        // to determine the line offset within this mapped region.
        let mut lines_past = 0u32;
        let entry_filename = &self.line_map_filenames[entry.filename_idx as usize];
        let file_content = if !self.files.is_empty() {
            self.files[0].content.as_bytes()
        } else {
            return SourceLocation {
                file: entry_filename.clone(),
                line: entry.orig_line,
                column: 1,
            };
        };

        let start = entry.pp_offset as usize;
        let end = offset as usize;
        if end <= file_content.len() && start <= end {
            for &b in &file_content[start..end] {
                if b == b'\n' {
                    lines_past += 1;
                }
            }
        }

        // Compute column: distance from the start of the current line.
        // Uses files[0].line_offsets (computed once during add_file) instead of
        // maintaining a separate pp_line_offsets copy.
        let line_offsets = &self.files[0].line_offsets;
        let col = if !line_offsets.is_empty() {
            let pp_line = match line_offsets.binary_search(&offset) {
                Ok(i) => i,
                Err(i) => if i > 0 { i - 1 } else { 0 },
            };
            offset.saturating_sub(line_offsets[pp_line]) + 1
        } else {
            1
        };

        SourceLocation {
            file: entry_filename.clone(),
            line: entry.orig_line + lines_past,
            column: col,
        }
    }

    /// Get the source line text for a given span (for error snippet display).
    /// Returns the full line containing the span start position.
    /// Assumes files[0] contains the preprocessed output (set by the driver via add_file).
    pub fn get_source_line(&self, span: Span) -> Option<String> {
        if self.files.is_empty() {
            return None;
        }
        let content = self.files[0].content.as_bytes();
        let offset = span.start as usize;
        if offset >= content.len() {
            return None;
        }

        // Find start of the line
        let mut line_start = offset;
        while line_start > 0 && content[line_start - 1] != b'\n' {
            line_start -= 1;
        }

        // Find end of the line
        let mut line_end = offset;
        while line_end < content.len() && content[line_end] != b'\n' {
            line_end += 1;
        }

        let line_bytes = &content[line_start..line_end];

        // Skip line markers (# <digit>... "filename" pattern), but not
        // other preprocessor directives like #define or #if which are valid
        // source lines that users may want to see in error snippets.
        if is_line_marker(line_bytes) {
            return None;
        }

        std::str::from_utf8(line_bytes).ok().map(|s| s.to_string())
    }
}

/// Check if a line (as bytes) is a GCC-style line marker: # <digit>... "filename"
/// Returns true only for line markers, not for preprocessor directives like #define.
fn is_line_marker(line: &[u8]) -> bool {
    let mut i = 0;
    // Skip leading whitespace
    while i < line.len() && line[i] == b' ' {
        i += 1;
    }
    // Must start with '#'
    if i >= line.len() || line[i] != b'#' {
        return false;
    }
    i += 1;
    // Skip whitespace after '#'
    while i < line.len() && line[i] == b' ' {
        i += 1;
    }
    // Next character must be a digit (this distinguishes line markers from directives)
    i < line.len() && line[i].is_ascii_digit()
}

fn compute_line_offsets(content: &str) -> Vec<u32> {
    let bytes = content.as_bytes();
    let len = bytes.len();
    // Pre-allocate: estimate ~60 bytes per line (typical for C code)
    let mut offsets = Vec::with_capacity(len / 60 + 1);
    offsets.push(0u32);
    let mut pos = 0;
    while pos < len {
        // Use memchr-style scanning: check bytes in chunks for newlines.
        // This is faster than enumerate() because it avoids the tuple overhead
        // and lets the compiler vectorize the inner search.
        if let Some(rel) = memchr_newline(&bytes[pos..]) {
            offsets.push((pos + rel + 1) as u32);
            pos += rel + 1;
        } else {
            break;
        }
    }
    offsets
}

/// Fast newline search. Returns the position of the first b'\n' in `haystack`,
/// or None if not found. Uses a simple loop that the compiler can auto-vectorize.
#[inline]
fn memchr_newline(haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == b'\n')
}

/// Parse a u32 directly from ASCII digit bytes, avoiding from_utf8 + parse overhead.
#[inline]
fn parse_u32_from_digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut result: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(result)
}
