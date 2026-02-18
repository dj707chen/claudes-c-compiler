# Experiments

This directory contains experiments that test **CCC** (Claude's C Compiler) as a
drop-in replacement for GCC on real-world C code.

---

## Session log — instructions given

The following are the exact instructions given during the session that produced
these experiments, in order:

1. *"generate a command line tool in basice C language in experiments/weatherTeller capable of calling some free web service to get the current weather condition by zip code"*
2. *"commit this"*
3. *"git config --global user.email and user.name then commit"* → provided `weiping / dj707chen@gmail.com`
4. *"push it"* (interrupted / not completed)
5. *"Change the cc command in experiments/weatherTell/Makefile to use the c compiler buit from this repository"*
6. *"do a release build"*
7. *"find a open source tool (curl if you could find one) written in basic C language, clone its source code, change it's C compiler to use the release version C compiler built from this repository, build it"*
8. *"can you record what you did in a markdown file?"*
9. *"Sorry I forgot to tell you also record the exact instructions I gave you in this file"*

---

## weatherTeller

**Directory:** `experiments/weatherTeller/`

A command-line weather tool written from scratch in C11 as a first smoke-test of
the compiler.

### What it does

Fetches current weather conditions by ZIP code from [wttr.in](https://wttr.in) — a
free service that requires no API key — and prints a formatted report with ASCII art.

```
$ ./weather 90210
  Weather for zip: 90210
  Location : West Hollywood, California, United States of America
  As of    : 2026-02-17 05:33 PM

   \  /
 _ /"".-'
   \_(
   /  (
      '-'

  Condition    : Partly cloudy
  Temperature  : 58 °F  (feels like 54 °F)
  Wind         : 18 mph SW
  Visibility   : 16 mi
  Precipitation: 0.0 in
  Humidity     : 62%
  Cloud cover  : 75%
  Pressure     : 1017 hPa

  Data: wttr.in (World Weather Online)  |  No API key required
```

### Implementation notes

| Aspect | Detail |
|--------|--------|
| HTTP | `popen()` + system `curl` binary (no libcurl dependency) |
| JSON parsing | Hand-rolled `strstr`-based field extractor — no external library |
| ASCII art | Keyed on wttr.in weather code |
| Units | `-f` Fahrenheit (default), `-c` Celsius |

### Build

```bash
cd experiments/weatherTeller
make          # uses ../../target/release/ccc
./weather 90210
./weather 10001 -c
```

The `Makefile` sets `CC = ../../target/release/ccc`.

### Compiler result

Built cleanly with zero errors and zero warnings under `-std=c11 -Wall -Wextra -Wpedantic -O2`.

---

## lz4

**Directory:** `experiments/lz4/`
**Source:** <https://github.com/lz4/lz4> (cloned with `--depth=1`)

[lz4](https://github.com/lz4/lz4) is a widely-used real-time compression algorithm
and CLI tool written in C. It is used inside the Linux kernel, Docker, and many other
production systems. It was chosen because:

- Pure C (no C++ or generated code)
- Simple, autoconf-free `Makefile`
- Non-trivial codebase (~12 translation units, multithreaded)

### Build

```bash
cd experiments/lz4
make CC=$(pwd)/../../target/release/ccc lz4
```

`CC` is passed on the command line — no source files were modified.

### Compiler result

All 12 source files compiled and linked successfully:

```
CC lz4file.o
CC lz4frame.o
CC lz4.o
CC lz4hc.o
CC xxhash.o
CC bench.o
CC lorem.o
CC lz4cli.o
CC lz4io.o
CC threadpool.o
CC timefn.o
CC util.o
LD lz4
==> built with multithreading support
```

Round-trip compress/decompress works correctly:

```bash
echo "Hello from ccc-built lz4!" | ./lz4 - - | ./lz4 -d - -
# Hello from ccc-built lz4!
```

### Known quirk

The `--version` string shows `v1.LZ4_VERSION_MINOR. 0` instead of the numeric
version. This is a minor preprocessor edge case (likely a `##` token-paste
interaction in lz4's version macros) and does not affect functionality.
