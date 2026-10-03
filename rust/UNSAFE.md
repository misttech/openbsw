# Unsafe code

The count of `unsafe` blocks, functions and impls per crate, outside tests, and why each
exists.

| Crate | `unsafe` | Why |
|---|---|---|
| `util` | 15 | `cell::RacyCell` (5: one `unsafe impl Sync`, two `unsafe fn`, two blocks): globals written during single-threaded startup and read afterwards, the contract the C++ globals have. `command::CommandNode` (2) and `ParentCommand` (2): the command list's links, changed only while adding commands at startup. `as_static` in `SimpleCommand`, `GroupCommand`, `HelpCommand` (3): a command names itself as the `'static` object the tree requires. `logger::Logger` (3): the facade's binding, set by `init`/`shutdown` and read by `log`. |
| `timer` | 2 | `unsafe impl Sync` for `TimerLink` and `Timer`: their cells are accessed only inside the timer's `Lock`, the platform's critical section, as the C++ list is. |
| `async` | 6 | `unsafe impl Sync` for `QueueNode`, `Queue`, `Timeout` and `EventDispatcher` (4): accessed only under the platform lock. `binding` (2): the installed `Async`, set once at startup and read afterwards. |
| `lifecycle` | 5 | `unsafe impl Sync` for `ComponentBase`, `ListenerNode`, `TransitionExecutor` and `LifecycleManager` (4): state written on the manager's context or during startup, read elsewhere, the same unsynchronized handshake the C++ manager uses. `as_static` (1): the manager, a `'static` object, re-queues itself as a runnable. |
| `logger` | 3 | `EntryBuffer` (2): `unsafe impl Sync` and the block that hands out its byte array, both under the contract that the owner holds its critical section around every call, as `BufferedLoggerOutput` does. `EntryCursor` (1): the draining task's reader position. |
| `cpp2can` | 6 | `unsafe impl Sync` for `ListenerNode`, `SentListenerNode` and `AbstractCanTransceiver` (3): the listener lists and state, changed under the transceiver's lock or on its receiving context. `unsafe impl Sync` for `BitFieldFilter`, `IntervalFilter` and `MaskFilter` (3): a filter is configured during startup or under the transceiver's lock, the C++ contract for a listener's filter. |
| `runtime` | 7 | `unsafe impl Sync` for `RuntimeStatistics`, `FunctionRuntimeStatistics`, `SimpleRuntimeEntry`, `NestedRuntimeEntry`, `RuntimeStack`, `StatisticsContainer` and `RuntimeMonitor`: all updated under the monitor's lock, the platform's critical section, as the C++ entries are from the context-switch hooks. |
| `console` | 4 | `unsafe impl Sync` for `ConsoleOutput`, `AsyncConsole`, `AsyncCommandWrapper` and `StdioConsoleInput`: the console has one line in flight, and its input is suspended while that line's command runs on another context, so one context at a time touches their cells, the contract the C++ console relies on. |
