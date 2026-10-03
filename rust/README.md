# OpenBSW in Rust

A port of Eclipse OpenBSW's libraries to Rust, one crate per upstream library under
`libs/bsw`, on the `main-mss` branch of this fork. The crates are `#![no_std]`,
allocation-free, and have no external dependencies, so they build for a bare-metal
target and run their tests on the host. They are translated directly from the C++
sources, with the same data structures, algorithms, and observable behavior, so that a
firmware built from them is indistinguishable from the C++ build to an observer outside
the chip: Forkpoint runs both and compares every console line and CAN frame with
`fpt equiv`.

The Zephyr glue and the demo application live in the `rust/` tree of
[openbsw-zephyr](https://github.com/misttech/openbsw-zephyr) (branch `main-mss`), which
depends on these crates by relative path: both trees are checked out side by side, as
`west` and Forkpoint's `prebuilt/third_party/` do.

| Crate | Ports | Contents |
|---|---|---|
| `util` | `libs/bsw/util` | printf formatting (`PrintfFormatter`, `StringWriter`, VT100 attributes), byte output streams, the console command tree, the `Logger` facade |
| `timer` | `libs/bsw/timer` | a sorted intrusive list of one-shot and cyclic timeouts over a wrapping 32-bit clock |
| `async` | `libs/bsw/async`, `asyncImpl`, the adaptation's `async/Types.h` | runnables and their intrusive queue, event dispatching, the `Timeout`, critical sections, the platform binding (`Async`), and a scripted `MockAsync` for tests (feature `mock`) |
| `lifecycle` | `libs/bsw/lifecycle` | run levels, components (`ComponentBase` merges the C++ base classes), the `LifecycleManager` with the exact transition order and log lines, listeners |
| `logger` | `libs/bsw/logger`, `loggerIntegration` | the ring of serialized entries (`EntryBuffer`, `BufferedLoggerOutput`), the component table and config, `ConsoleEntryFormatter` producing `<ms>: Core0: <component>: <level>: <message>`, the console output and the composition that drains one entry per run |
| `cpp2can` | `libs/bsw/cpp2can` | `CanFrame` and the `can_id` encoding, the id filters (`BitFieldFilter`, `IntervalFilter`, `MaskFilter`, `StaticBitFieldFilter`) and their merger, the frame and sent-frame listeners, the `CanTransceiver` trait and the `AbstractCanTransceiver` base with the listener lists |

Strings are byte slices (`&[u8]`): console input is not guaranteed to be UTF-8, and the
C++ code works on `char const*`.

## Building and testing

```sh
cd rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --workspace --target thumbv7em-none-eabi --release
```

The last line builds the crates for a Cortex-M4 without an FPU, the target Zephyr's
`nucleo_g474re` board uses; `rustup target add thumbv7em-none-eabi` installs it.

See `PORTING.md` for how a library is ported and `UNSAFE.md` for the `unsafe` count per
crate.
