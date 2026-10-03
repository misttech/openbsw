# Porting a library

Ports follow Forkpoint's C and C++ to Rust rubric (`.agents/skills/cpp-to-rust-rubric`
in the Forkpoint repository). In short:

1. **One library, one crate; one source file, one module.** Translate with the same data
   structures and algorithms. Every public class, function, and constant has a Rust
   counterpart with the same behavior, corner cases included: a null string prints
   `<NULL>`, a missing argument `<?>`, `%%` prints `%`, and a `0238` token scans as an
   octal `023` followed by a stray `8`.
2. **Globals stay global.** C++ objects constructed at file scope become `static` items
   with `const fn` constructors. State that C++ sets up before the tasks run and only
   reads afterwards (the logger binding, the sorted command list) lives in a `RacyCell`,
   whose documented contract is exactly that. Everything else uses atomics or a critical
   section.
3. **No new failure modes.** Intrusive lists stay intrusive, so nothing is "full" that was
   not full in C++. No allocation, no panics on paths the C++ code could not fail on, no
   arithmetic overflow the C++ code did not have (the tokenizer wraps as the C++
   template does on unsigned types). An `ETL_ASSERT` becomes a panic with the same
   message.
4. **Lifetimes replace protocols where they can.** `startOutput`/`endOutput` pairs
   become a closure passed to `with_output`, which cannot be left unbalanced; the
   `SharedStringWriter` whose constructor and destructor bracket an output becomes
   `with_shared_writer`. Where C++ compares object addresses (continuous users of a
   shared stream), Rust compares `'static` references by address.
5. **Test parity.** Every upstream `test/` case that applies becomes a Rust test in the
   same module, named after the C++ test; a case that depends on an API shape Rust does
   not have (writing to a stream after `endOutput`) is noted in the module. Code without
   C++ tests gains tests for its documented behavior and edge cases.
6. **Safety at the edge.** Raw pointers and `unsafe` appear only where a `'static` object
   must be named as such (`as_static`) or where the C++ global's contract is honored
   (`RacyCell`). Every `unsafe` block has a `// SAFETY:` comment and every `unsafe fn` a
   `# Safety` section. Update `UNSAFE.md`.
7. **Documentation.** Keep the C++ comments that document behavior, as doc comments on
   the Rust items. New files carry the Mist copyright line and the Apache-2.0 SPDX
   identifier; a direct translation also names its source in the module documentation.
8. **Formatting.** Format strings stay C printf strings (`b"%d"`), so log messages and
   console output are byte-identical to the C++ build. Arguments are passed as a slice of
   `Arg` values; the `log_*!` macros build it.
9. **Equivalence.** In Forkpoint, `make test-nucleo-g474re-openbsw-rust-demo` builds the
   demo from these crates and checks it against the C++ build with `fpt equiv`.
10. **Objects that name themselves.** A C++ member constructed with `*this` (a context's
    event policy, a transceiver's runnables, the console's help command) becomes a field
    built from a `this: &'static Self` parameter of the `const fn` constructor, and the
    `static` item passes its own address: `static CONSOLE: AsyncConsole =
    AsyncConsole::new(&CONSOLE)`. Where a C++ constructor registers the object somewhere
    (a command wrapper adding itself to the console), the application registers it at
    startup instead, next to its lifecycle components.
11. **Pools become slot arrays.** An `etl::generic_pool` member that hands out objects
    (DoCAN's message receivers and transmitters) becomes a fixed array of reusable slots
    plus an array of indices that keeps the C++ list order, sized by the same const
    parameters. A slot in use is handed out as a `'static` reference, which it is because
    the pool lives in a `static` config; nothing new can be "full" and nothing allocates.
12. **The version the demo is built with.** Where the fork's `main` has moved on from the
    OpenBSW commit the Zephyr demo pins (the transport router, UDS), the port follows the
    pinned commit, since that is the C++ image Forkpoint compares against; the crate's
    documentation names it. A request is a slice of the transport message's cells
    (`&[Cell<u8>]`): the C++ clears the suppress-positive-response bit in the request
    buffer as it dispatches, and so does the port.

