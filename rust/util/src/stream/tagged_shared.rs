// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A tagged shared stream, ported from `util/stream/TaggedSharedOutputStream`.
//!
//! Only the non-continuous variant is ported: the demo's console uses that one, and the
//! continuous variant registers the stream itself as a continuous user of the stream it
//! wraps, which needs a `'static` self.

use super::{ContinuousUser, OutputStream, SharedOutputStream, TaggedOutputHelper, same_user};

/// Tags the output of a shared stream with a prefix and suffix per line.
///
/// Each output ends its last line, unless the caller continues it as a user and
/// returns before anyone else writes.
pub struct TaggedSharedOutputStream<'s> {
    shared: &'s mut dyn SharedOutputStream,
    helper: TaggedOutputHelper<'s>,
    user: Option<&'static dyn ContinuousUser>,
}

struct TaggedWriter<'h, 's, 'w> {
    helper: &'h mut TaggedOutputHelper<'s>,
    stream: &'w mut dyn OutputStream,
}

impl OutputStream for TaggedWriter<'_, '_, '_> {
    fn is_eof(&self) -> bool {
        self.stream.is_eof()
    }

    fn write(&mut self, data: u8) {
        self.helper.write_bytes(self.stream, &[data]);
    }

    fn write_bytes(&mut self, buffer: &[u8]) {
        self.helper.write_bytes(self.stream, buffer);
    }
}

impl<'s> TaggedSharedOutputStream<'s> {
    /// Tag the lines written through `shared` with `prefix` and `suffix`.
    pub fn new(shared: &'s mut dyn SharedOutputStream, prefix: &'s [u8], suffix: &'s [u8]) -> Self {
        Self { shared, helper: TaggedOutputHelper::new(prefix, suffix), user: None }
    }
}

impl SharedOutputStream for TaggedSharedOutputStream<'_> {
    fn with_output(
        &mut self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    ) {
        let helper = &mut self.helper;
        let previous = self.user;
        self.shared.with_output(None, &mut |stream| {
            if user.is_none()
                || !same_user(
                    previous.map(|u| u as &dyn ContinuousUser),
                    user.map(|u| u as &dyn ContinuousUser),
                )
            {
                if let Some(previous) = previous {
                    previous.end_continuous_output(&mut TaggedWriter { helper, stream });
                }
                helper.end_line(stream);
            }
            f(&mut TaggedWriter { helper, stream });
            if user.is_none() {
                helper.end_line(stream);
            }
        });
        self.user = user;
    }

    fn release_continuous_user(&mut self, user: &dyn ContinuousUser) {
        if same_user(self.user.map(|u| u as &dyn ContinuousUser), Some(user)) {
            self.with_output(None, &mut |_| {});
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};

    struct NoopUser;

    impl ContinuousUser for NoopUser {
        fn end_continuous_output(&self, _stream: &mut dyn OutputStream) {}
    }

    static USER: NoopUser = NoopUser;
    static OTHER: NoopUser = NoopUser;

    // Ported from util/test/src/util/stream/TaggedSharedOutputStreamTest.cpp.
    #[test]
    fn prefix_and_suffixes_are_inserted() {
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let mut cut = TaggedSharedOutputStream::new(&mut shared, b"[START]", b"[CRLF]");
            cut.with_output(None, &mut |s| {
                s.write(b'a');
                s.write(b'\n');
                s.write_bytes(b"abc\ndef");
                s.write_bytes(b"AB\nDEF");
            });
        }
        assert_eq!(
            stream.string(),
            b"[START]a[CRLF][START]abc[CRLF][START]defAB[CRLF][START]DEF[CRLF]"
        );
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let mut cut = TaggedSharedOutputStream::new(&mut shared, b"[START]", b"[CRLF]");
            cut.with_output(None, &mut |s| s.write(b'\n'));
        }
        assert_eq!(stream.string(), b"[START][CRLF]");
    }

    #[test]
    fn non_continuous_stream_continues_a_user_line() {
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let mut cut = TaggedSharedOutputStream::new(&mut shared, b"[START]", b"[CRLF]");
            cut.with_output(Some(&USER), &mut |s| {
                s.write(b'a');
                s.write(b'b');
            });
            cut.with_output(Some(&USER), &mut |s| s.write(b'c'));
            cut.release_continuous_user(&OTHER);
            cut.release_continuous_user(&USER);
        }
        assert_eq!(stream.string(), b"[START]abc[CRLF]");
    }

    #[test]
    fn eof_is_reported_correctly() {
        let mut buffer = [0u8; 10];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let mut shared = SharedOutputStreamImpl::new(&mut stream);
        let mut cut = TaggedSharedOutputStream::new(&mut shared, b"", b"[CRLF]");
        cut.with_output(None, &mut |s| {
            assert!(!s.is_eof());
            s.write(b'a');
            s.write(b'b');
            s.write(b'\n');
            assert!(!s.is_eof());
            s.write(b'a');
            assert!(s.is_eof());
        });
    }
}
