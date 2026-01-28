#!/bin/bash
# Setup i686 cross-compilation environment for building projects with C++ deps.
#
# The standard Docker/CI environment provides i686-linux-gnu-gcc but not
# i686-linux-gnu-g++ (the C++ cross-compiler). This script creates:
#
# 1. A wrapper i686-linux-gnu-g++ in a configurable location that handles C++
#    compilation for known libraries (e.g. Redis's fast_float) by substituting
#    a pure-C fallback compiled with i686-linux-gnu-gcc.
#
# 2. /usr/i686-linux-gnu/lib/libstdc++.so - Symlink to libstdc++.so.6 so
#    that -lstdc++ resolves during linking.
#
# Usage:
#   sudo ./scripts/setup_i686_cross.sh              # installs to /usr/local/bin
#   ./scripts/setup_i686_cross.sh ~/.cargo/bin       # installs to user dir
#
# This only needs to be run once per environment.

set -e

# Default to /usr/local/bin, allow override via first argument
INSTALL_DIR="${1:-/usr/local/bin}"
WRAPPER="$INSTALL_DIR/i686-linux-gnu-g++"
LIBDIR=/usr/i686-linux-gnu/lib

# 1. Create i686-linux-gnu-g++ wrapper
if command -v i686-linux-gnu-g++ >/dev/null 2>&1; then
    echo "i686-linux-gnu-g++ already exists on PATH, skipping wrapper creation."
else
    echo "Creating i686-linux-gnu-g++ wrapper at $WRAPPER..."
    mkdir -p "$INSTALL_DIR"
    cat > "$WRAPPER" << 'WRAPPER_EOF'
#!/bin/bash
# Wrapper for i686-linux-gnu-g++ when the real cross-compiler is not installed.
#
# For C++ source files: generates a C fallback stub (providing the same
# exported symbols) and compiles it with i686-linux-gnu-gcc. This avoids
# needing i686 C++ headers which are not available in the cross environment.
#
# Currently handles:
# - fast_float_strtod.cpp (Redis dependency) -> strtod() wrapper

compile_mode=false
has_cpp_input=false
output_file=""
prev_was_o=false

for arg in "$@"; do
    case "$arg" in
        -c) compile_mode=true ;;
        -o) prev_was_o=true; continue ;;
        *.cpp|*.cc|*.cxx|*.C) has_cpp_input=true ;;
    esac
    if $prev_was_o; then
        output_file="$arg"
        prev_was_o=false
    fi
done

if $compile_mode && $has_cpp_input; then
    # Compiling C++ source - provide a C fallback stub and compile with
    # the i686 C cross-compiler. This handles one source file at a time
    # (matching how make invokes the compiler for each .cpp file).
    cpp_src=""
    other_args=()
    for arg in "$@"; do
        case "$arg" in
            *.cpp|*.cc|*.cxx|*.C) cpp_src="$arg" ;;
            -std=c++*) ;; # strip C++ std flags
            *) other_args+=("$arg") ;;
        esac
    done

    base=$(basename "$cpp_src" | sed 's/\.[^.]*$//')
    stub="/tmp/cxx_stub_${base}_$$.c"

    if echo "$cpp_src" | grep -q "fast_float"; then
        cat > "$stub" << 'CSTUB'
#include <stdlib.h>
#include <errno.h>
double fast_float_strtod(const char *nptr, char **endptr) {
    return strtod(nptr, endptr);
}
CSTUB
    else
        # TODO: Unknown C++ file - only a warning stub is provided.
        # Add specific stubs here as new C++ dependencies are encountered.
        echo "Warning: i686-linux-gnu-g++ wrapper: unknown C++ file '$cpp_src', creating empty stub" >&2
        echo "/* C++ stub - no symbols */" > "$stub"
    fi

    # Use -o from args if provided, otherwise default to ${base}.o
    if [ -z "$output_file" ]; then
        output_file="${base}.o"
    fi

    # Replace the source file with our stub in the args
    filtered=(-c)
    for a in "${other_args[@]}"; do
        case "$a" in
            -c) ;; # already added
            -o) ;; # handled separately
            *) filtered+=("$a") ;;
        esac
    done

    i686-linux-gnu-gcc "${filtered[@]}" "$stub" -o "$output_file"
    ret=$?
    rm -f "$stub"
    exit $ret
else
    # Non-compile mode (linking etc.) - delegate to i686 gcc, strip -lstdc++
    filtered=()
    for arg in "$@"; do
        [ "$arg" != "-lstdc++" ] && filtered+=("$arg")
    done
    exec i686-linux-gnu-gcc "${filtered[@]}"
fi
WRAPPER_EOF
    chmod +x "$WRAPPER"
    echo "  Created $WRAPPER"
fi

# 2. Create libstdc++.so symlink for -lstdc++ to resolve
if [ -d "$LIBDIR" ] && [ ! -e "$LIBDIR/libstdc++.so" ]; then
    if [ -e "$LIBDIR/libstdc++.so.6" ]; then
        echo "Creating libstdc++.so symlink in $LIBDIR..."
        ln -sf libstdc++.so.6 "$LIBDIR/libstdc++.so"
        echo "  Created $LIBDIR/libstdc++.so -> libstdc++.so.6"
    else
        echo "Warning: $LIBDIR/libstdc++.so.6 not found, skipping symlink."
    fi
else
    if [ -e "$LIBDIR/libstdc++.so" ]; then
        echo "libstdc++.so symlink already exists, skipping."
    else
        echo "Warning: $LIBDIR does not exist, skipping libstdc++ symlink."
    fi
fi

echo "i686 cross-compilation environment setup complete."
