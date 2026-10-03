// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Text formatting, ported from `util/format`.
//!
//! [`PrintfFormatter`] renders a printf-style format string into an output stream.
//! [`StringWriter`] is the chaining front end most code uses, and
//! [`Vt100AttributedStringFormatter`] adds terminal colors and styles.

mod attributed;
mod formatter;
mod printf;
mod scanner;
mod string_writer;
mod vt100;

pub use attributed::{
    AttributedString, BLINK, BOLD, Color, DIM, HIDDEN, NUMBER_OF_FORMATS, REVERSE,
    StringAttributes, UNDERLINE,
};
pub use formatter::PrintfFormatter;
pub use printf::{
    Arg, ArgumentReader, ParamDatatype, ParamInfo, ParamType, ParamVariant, ParamWidthOrPrecision,
    SliceArgumentReader, flags,
};
pub use scanner::{PrintfFormatScanner, TokenType};
pub use string_writer::{Manipulator, StringWriter, with_shared_writer};
pub use vt100::Vt100AttributedStringFormatter;
