// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The plain shared stream, ported from `util/stream/SharedOutputStream`.

use super::{ContinuousUser, OutputStream, SharedOutputStream, same_user};

/// Shares one output stream, honoring the continuous-user protocol.
pub struct SharedOutputStreamImpl<'s> {
    stream: &'s mut dyn OutputStream,
    user: Option<&'static dyn ContinuousUser>,
}

impl<'s> SharedOutputStreamImpl<'s> {
    /// Share `stream`.
    pub fn new(stream: &'s mut dyn OutputStream) -> Self {
        Self { stream, user: None }
    }
}

impl SharedOutputStream for SharedOutputStreamImpl<'_> {
    fn with_output(
        &mut self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    ) {
        if let Some(previous) = self.user
            && !same_user(Some(previous), user.map(|u| u as &dyn ContinuousUser))
        {
            previous.end_continuous_output(self.stream);
        }
        f(self.stream);
        self.user = user;
    }

    fn release_continuous_user(&mut self, user: &dyn ContinuousUser) {
        if let Some(current) = self.user
            && same_user(Some(current), Some(user))
        {
            self.user = None;
            user.end_continuous_output(self.stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StringBufferOutputStream;
    use core::cell::Cell;

    struct TestUser {
        end_count: Cell<u32>,
    }

    impl ContinuousUser for TestUser {
        fn end_continuous_output(&self, stream: &mut dyn OutputStream) {
            self.end_count.set(self.end_count.get() + 1);
            stream.write(b'|');
        }
    }

    fn leak_user() -> &'static TestUser {
        std::boxed::Box::leak(std::boxed::Box::new(TestUser { end_count: Cell::new(0) }))
    }

    // Ported from util/test/src/util/stream/SharedOutputStreamTest.cpp; the stream
    // identity checks become checks of what reaches the stream.
    #[test]
    fn stream_is_handed_out() {
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = SharedOutputStreamImpl::new(&mut stream);
            cut.with_output(None, &mut |s| s.write(b'a'));
            cut.with_output(None, &mut |s| s.write(b'b'));
        }
        assert_eq!(stream.string(), b"ab");
    }

    #[test]
    fn user_is_obeyed() {
        let user = leak_user();
        let other = leak_user();
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = SharedOutputStreamImpl::new(&mut stream);
            cut.with_output(Some(user), &mut |s| s.write(b'a'));
            // Another output ends the continuous one first.
            cut.with_output(None, &mut |s| s.write(b'b'));
            assert_eq!(user.end_count.get(), 1);
            cut.with_output(Some(user), &mut |s| s.write(b'c'));
            cut.release_continuous_user(other);
            assert_eq!(user.end_count.get(), 1);
            cut.release_continuous_user(user);
            assert_eq!(user.end_count.get(), 2);
        }
        assert_eq!(stream.string(), b"a|bc|");
    }

    #[test]
    fn continuing_user_is_not_notified() {
        let user = leak_user();
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = SharedOutputStreamImpl::new(&mut stream);
            cut.with_output(Some(user), &mut |s| s.write(b'a'));
            cut.with_output(Some(user), &mut |s| s.write(b'b'));
            assert_eq!(user.end_count.get(), 0);
        }
        assert_eq!(stream.string(), b"ab");
    }
}
