// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The intrusive FIFO, ported from `async/Queue.h` and `async/QueueNode.h`.

use core::cell::Cell;

/// The link a queued object embeds: the port of `QueueNode<T>`.
///
/// C++ marks "not enqueued" with the pointer value 1 and "last in the queue" with null;
/// the link is the same three-way state.
pub struct QueueNode<T: ?Sized + 'static> {
    link: Cell<Link<T>>,
}

enum Link<T: ?Sized + 'static> {
    NotEnqueued,
    Last,
    Next(&'static T),
}

impl<T: ?Sized + 'static> Clone for Link<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized + 'static> Copy for Link<T> {}

// SAFETY: a node's link is read and written only while its queue's owner holds the
// platform lock, as the C++ executor does; the type adds no sharing of its own.
unsafe impl<T: ?Sized + Sync> Sync for QueueNode<T> {}

impl<T: ?Sized + 'static> QueueNode<T> {
    /// A node that is not enqueued.
    pub const fn new() -> Self {
        Self { link: Cell::new(Link::NotEnqueued) }
    }

    /// Whether the node is in a queue.
    pub fn is_enqueued(&self) -> bool {
        !matches!(self.link.get(), Link::NotEnqueued)
    }

    /// The next node, `None` for the last one (or when not enqueued).
    pub fn next(&self) -> Option<&'static T> {
        match self.link.get() {
            Link::Next(next) => Some(next),
            Link::NotEnqueued | Link::Last => None,
        }
    }

    /// Link `next` after this node; `None` makes it the last.
    pub fn set_next(&self, next: Option<&'static T>) {
        self.link.set(match next {
            Some(next) => Link::Next(next),
            None => Link::Last,
        });
    }

    /// Mark the node enqueued, as the last one.
    pub fn enqueue(&self) {
        self.link.set(Link::Last);
    }

    /// Mark the node not enqueued; returns the node that followed it.
    pub fn dequeue(&self) -> Option<&'static T> {
        let next = self.next();
        self.link.set(Link::NotEnqueued);
        next
    }
}

impl<T: ?Sized + 'static> Default for QueueNode<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An object that embeds a [`QueueNode`].
pub trait HasQueueNode {
    /// The node that links this object into a queue.
    fn queue_node(&self) -> &QueueNode<Self>;
}

/// A FIFO of `'static` objects linked through their nodes: the port of `Queue<Node>`.
pub struct Queue<T: ?Sized + HasQueueNode + 'static> {
    first: Cell<Option<&'static T>>,
    last: Cell<Option<&'static T>>,
}

// SAFETY: the queue is read and written only while its owner holds the platform lock;
// the type adds no sharing of its own.
unsafe impl<T: ?Sized + HasQueueNode + Sync> Sync for Queue<T> {}

fn same<T: ?Sized>(a: &T, b: &T) -> bool {
    core::ptr::addr_eq(a, b)
}

impl<T: ?Sized + HasQueueNode + 'static> Queue<T> {
    /// An empty queue.
    pub const fn new() -> Self {
        Self { first: Cell::new(None), last: Cell::new(None) }
    }

    /// Append `node`.
    pub fn enqueue(&self, node: &'static T) {
        node.queue_node().enqueue();
        match self.last.get() {
            Some(last) => last.queue_node().set_next(Some(node)),
            None => self.first.set(Some(node)),
        }
        self.last.set(Some(node));
    }

    /// Remove and return the first node.
    pub fn dequeue(&self) -> Option<&'static T> {
        let node = self.first.get()?;
        if self.last.get().is_some_and(|last| same(last, node)) {
            self.last.set(None);
        }
        self.first.set(node.queue_node().dequeue());
        Some(node)
    }

    /// Remove every node.
    pub fn clear(&self) {
        while let Some(first) = self.first.get() {
            self.first.set(first.queue_node().dequeue());
        }
        self.last.set(None);
    }

    /// Remove `node` from wherever it is; nothing happens if it is not queued here.
    pub fn remove(&self, node: &T) {
        let mut prev: Option<&'static T> = None;
        let mut current = self.first.get();
        while let Some(candidate) = current {
            if same(candidate, node) {
                let next = candidate.queue_node().dequeue();
                match prev {
                    Some(prev) => prev.queue_node().set_next(next),
                    None => self.first.set(next),
                }
                if self.last.get().is_some_and(|last| same(last, candidate)) {
                    self.last.set(prev);
                }
                break;
            }
            prev = Some(candidate);
            current = candidate.queue_node().next();
        }
    }
}

impl<T: ?Sized + HasQueueNode + 'static> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestNode {
        node: QueueNode<TestNode>,
    }

    impl HasQueueNode for TestNode {
        fn queue_node(&self) -> &QueueNode<TestNode> {
            &self.node
        }
    }

    fn node() -> &'static TestNode {
        std::boxed::Box::leak(std::boxed::Box::new(TestNode { node: QueueNode::new() }))
    }

    // Ported from asyncImpl/test/src/async/QueueNodeTest.cpp.
    #[test]
    fn queue_node() {
        let cut = node();
        assert!(!cut.node.is_enqueued());
        cut.node.enqueue();
        assert!(cut.node.is_enqueued());
        assert!(cut.node.next().is_none());
        assert!(cut.node.dequeue().is_none());
        assert!(!cut.node.is_enqueued());
        let next = node();
        cut.node.enqueue();
        assert!(cut.node.is_enqueued());
        cut.node.set_next(Some(next));
        assert!(cut.node.is_enqueued());
        assert!(cut.node.next().is_some_and(|n| same(n, next)));
        cut.node.set_next(None);
        assert!(cut.node.is_enqueued());
        assert!(cut.node.next().is_none());
        cut.node.enqueue();
        cut.node.set_next(Some(next));
        assert!(cut.node.dequeue().is_some_and(|n| same(n, next)));
        assert!(!cut.node.is_enqueued());
    }

    // Ported from asyncImpl/test/src/async/QueueTest.cpp.
    #[test]
    fn queue() {
        let cut: Queue<TestNode> = Queue::new();
        let node1 = node();
        let node2 = node();
        let node3 = node();
        assert!(cut.dequeue().is_none());
        cut.enqueue(node1);
        assert!(node1.node.is_enqueued());
        assert!(cut.dequeue().is_some_and(|n| same(n, node1)));
        assert!(!node1.node.is_enqueued());
        assert!(cut.dequeue().is_none());
        // Several nodes come out in order.
        cut.enqueue(node1);
        cut.enqueue(node2);
        assert!(cut.dequeue().is_some_and(|n| same(n, node1)));
        cut.enqueue(node3);
        assert!(cut.dequeue().is_some_and(|n| same(n, node2)));
        assert!(cut.dequeue().is_some_and(|n| same(n, node3)));
        assert!(!node3.node.is_enqueued());
        assert!(cut.dequeue().is_none());
        // Clear.
        cut.enqueue(node1);
        cut.enqueue(node2);
        cut.clear();
        assert!(!node1.node.is_enqueued());
        assert!(!node2.node.is_enqueued());
        assert!(cut.dequeue().is_none());
        // Remove the first node.
        cut.enqueue(node1);
        cut.enqueue(node2);
        cut.remove(node1);
        assert!(!node1.node.is_enqueued());
        assert!(node2.node.is_enqueued());
        assert!(cut.dequeue().is_some_and(|n| same(n, node2)));
        assert!(cut.dequeue().is_none());
        // Remove the last node.
        cut.enqueue(node1);
        cut.enqueue(node2);
        cut.remove(node2);
        assert!(node1.node.is_enqueued());
        assert!(!node2.node.is_enqueued());
        assert!(cut.dequeue().is_some_and(|n| same(n, node1)));
        assert!(cut.dequeue().is_none());
        // Remove an inner node.
        cut.enqueue(node1);
        cut.enqueue(node2);
        cut.enqueue(node3);
        cut.remove(node2);
        assert!(cut.dequeue().is_some_and(|n| same(n, node1)));
        assert!(cut.dequeue().is_some_and(|n| same(n, node3)));
        assert!(cut.dequeue().is_none());
        // Remove a node that is not enqueued.
        cut.remove(node1);
    }
}
