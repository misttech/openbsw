// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Byte output streams, ported from `util/stream`.
//!
//! [`OutputStream`] is the sink everything writes to. [`SharedOutputStream`] hands out
//! exclusive access to a stream for the duration of one output; in C++ that is a
//! `startOutput`/`endOutput` pair, here it is a closure passed to
//! [`SharedOutputStream::with_output`], which cannot be left unbalanced.

mod byte_buffer;
mod normalize_lf;
mod null;
mod shared;
mod stdout;
mod string_buffer;
mod tagged;
mod tagged_helper;
mod tagged_shared;

pub use byte_buffer::ByteBufferOutputStream;
pub use normalize_lf::NormalizeLfOutputStream;
pub use null::NullOutputStream;
pub use shared::SharedOutputStreamImpl;
pub use stdout::{Stdio, StdoutStream};
pub use string_buffer::StringBufferOutputStream;
pub use tagged::TaggedOutputStream;
pub use tagged_helper::TaggedOutputHelper;
pub use tagged_shared::TaggedSharedOutputStream;

/// A sink for bytes: the port of `IOutputStream`.
pub trait OutputStream {
    /// Whether the end of the stream has been reached, so that no more bytes can be
    /// written.
    fn is_eof(&self) -> bool;

    /// Write one byte.
    fn write(&mut self, data: u8);

    /// Write a block of bytes. The default writes them one by one.
    fn write_bytes(&mut self, buffer: &[u8]) {
        for &data in buffer {
            self.write(data);
        }
    }
}

/// A user of a shared stream that wants to continue its output across several outputs:
/// the port of `ISharedOutputStream::IContinuousUser`.
///
/// Users are told apart by address, so one that registers must live for the program.
pub trait ContinuousUser {
    /// Called when the user ended its output as continuous and another user takes the
    /// stream, so it can finish its line.
    fn end_continuous_output(&self, stream: &mut dyn OutputStream);
}

/// A stream shared between several writers: the port of `ISharedOutputStream`.
pub trait SharedOutputStream {
    /// Start the output, run `f` with the stream, and end the output.
    ///
    /// `user` is the continuous user that wants to continue its output on the next call,
    /// or `None`. If another user takes the stream in between, the user's
    /// [`ContinuousUser::end_continuous_output`] is called first.
    fn with_output(
        &mut self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    );

    /// Remove pending references to a continuous user. Called when the user becomes
    /// invalid after it ended an output as continuous; the user may be called back to end
    /// its output.
    fn release_continuous_user(&mut self, user: &dyn ContinuousUser);
}

/// Whether two continuous users are the same object.
pub(crate) fn same_user(a: Option<&dyn ContinuousUser>, b: Option<&dyn ContinuousUser>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => core::ptr::addr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
