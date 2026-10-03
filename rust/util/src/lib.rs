// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of Eclipse OpenBSW's `libs/bsw/util`: the parts the demo application and the
//! other ported libraries use.
//!
//! - [`format`]: the printf engine (`PrintfFormatScanner`, `PrintfFormatter`), the
//!   `StringWriter`, and VT100 attributed strings.
//! - [`stream`]: byte output streams, shared streams with the continuous-user protocol,
//!   tagged (prefix/suffix per line) and LF-normalizing streams.
//! - [`command`]: the console command tree (`ParentCommand`, `SimpleCommand`,
//!   `GroupCommand`, `HelpCommand`) and the `CommandContext` tokenizer.
//! - [`logger`]: the `Logger` facade, log levels, and component and level infos.
//! - [`string`]: byte-string helpers standing in for `util::string::ConstString`.
//!
//! Strings are byte slices (`&[u8]`): console input is not guaranteed to be UTF-8, and
//! the C++ code works on `char const*`. `&str` converts with `as_bytes()`.
//!
//! The crate is `no_std` and allocation-free, as the C++ library is.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(test)]
extern crate std;

pub mod cell;
pub mod command;
pub mod format;
pub mod logger;
pub mod stream;
pub mod string;
