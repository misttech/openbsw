# Unsafe code

The count of `unsafe` blocks, functions and impls per crate, outside tests, and why each
exists.

| Crate | `unsafe` | Why |
|---|---|---|
| `util` | 15 | `cell::RacyCell` (5: one `unsafe impl Sync`, two `unsafe fn`, two blocks): globals written during single-threaded startup and read afterwards, the contract the C++ globals have. `command::CommandNode` (2) and `ParentCommand` (2): the command list's links, changed only while adding commands at startup. `as_static` in `SimpleCommand`, `GroupCommand`, `HelpCommand` (3): a command names itself as the `'static` object the tree requires. `logger::Logger` (3): the facade's binding, set by `init`/`shutdown` and read by `log`. |
