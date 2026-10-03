// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The dispatcher's configuration: the port of `DiagnosisConfiguration.h` (the connection
//! pool and the send queue), and of the application's `TransportConfiguration.h`.

use core::cell::Cell;

use openbsw_async::ContextType;
use openbsw_transport::{TransportMessage, TransportMessageProcessedListener};

use crate::connection::IncomingDiagConnection;

/// The addresses and sizes of the demo's `TransportConfiguration.h`.
#[derive(Clone, Copy, Debug)]
pub struct TransportConfiguration {
    /// The functional address every ECU answers (`FUNCTIONAL_ALL_ISO14229`).
    pub functional_address: u16,
    /// The 8-bit tester addresses (`TESTER_RANGE_8BIT_START..=END`).
    pub tester_range_8bit: (u16, u16),
    /// The DoIP tester addresses (`TESTER_RANGE_DOIP_START..=END`).
    pub tester_range_doip: (u16, u16),
    /// The largest functional request (`MAX_FUNCTIONAL_MESSAGE_PAYLOAD_SIZE`).
    pub max_functional_message_payload_size: u16,
    /// The largest diagnostic message (`DIAG_PAYLOAD_SIZE`).
    pub diag_payload_size: u16,
}

impl TransportConfiguration {
    /// The demo's values: functional address 0xDF, testers 0xF0..=0xFD and 0xEF0..=0xEFD,
    /// 6-byte functional requests and 4095-byte messages.
    pub const DEMO: Self = Self {
        functional_address: 0x00DF,
        tester_range_8bit: (0x00F0, 0x00FD),
        tester_range_doip: (0x0EF0, 0x0EFD),
        max_functional_message_payload_size: 6,
        diag_payload_size: 4095,
    };

    /// Whether `address` is the functional address.
    pub const fn is_functional_address(&self, address: u16) -> bool {
        address == self.functional_address
    }

    /// Whether `message` went to the functional address.
    pub fn is_functionally_addressed(&self, message: &TransportMessage) -> bool {
        self.is_functional_address(message.target_id())
    }

    /// Whether `address` is a tester's.
    pub const fn is_tester_address(&self, address: u16) -> bool {
        (address >= self.tester_range_8bit.0 && address <= self.tester_range_8bit.1)
            || (address >= self.tester_range_doip.0 && address <= self.tester_range_doip.1)
    }

    /// Whether `message` came from a tester.
    pub fn is_from_tester(&self, message: &TransportMessage) -> bool {
        self.is_tester_address(message.source_id())
    }
}

/// A message to dispatch and who to tell when it was (`transport::TransportJob`).
#[derive(Clone, Copy)]
pub struct TransportJob {
    /// The message.
    pub message: &'static TransportMessage,
    /// Who hears that it was processed.
    pub processed_listener: &'static dyn TransportMessageProcessedListener,
}

/// A ring of jobs (`etl::queue<TransportJob, N>`).
pub struct TransportJobQueue<const N: usize> {
    slots: [Cell<Option<TransportJob>>; N],
    head: Cell<usize>,
    len: Cell<usize>,
}

// SAFETY: changed under the platform lock, as the C++ queue is.
unsafe impl<const N: usize> Sync for TransportJobQueue<N> {}

impl<const N: usize> TransportJobQueue<N> {
    /// An empty queue.
    pub const fn new() -> Self {
        Self { slots: [const { Cell::new(None) }; N], head: Cell::new(0), len: Cell::new(0) }
    }

    /// Whether no job can be added.
    pub fn is_full(&self) -> bool {
        self.len.get() == N
    }

    /// Whether no job is queued.
    pub fn is_empty(&self) -> bool {
        self.len.get() == 0
    }

    /// Add `job` at the back; returns whether there was room.
    pub fn push(&self, job: TransportJob) -> bool {
        if self.is_full() {
            return false;
        }
        let index = (self.head.get() + self.len.get()) % N;
        self.slots[index].set(Some(job));
        self.len.set(self.len.get() + 1);
        true
    }

    /// Take the job at the front.
    pub fn pop(&self) -> Option<TransportJob> {
        if self.is_empty() {
            return None;
        }
        let job = self.slots[self.head.get()].take();
        self.head.set((self.head.get() + 1) % N);
        self.len.set(self.len.get() - 1);
        job
    }
}

impl<const N: usize> Default for TransportJobQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What the dispatcher asks of its configuration, whatever its sizes.
pub trait DiagnosisConfig: Sync {
    /// This ECU's address.
    fn diag_address(&self) -> u16;
    /// The functional address, or `TransportMessage::INVALID_ADDRESS` for none.
    fn broadcast_address(&self) -> u16;
    /// The bus the dispatcher is a layer of.
    fn diag_bus_id(&self) -> u8;
    /// The largest answer.
    fn max_response_payload_size(&self) -> u16;
    /// Whether response-pending answers go out while a job takes its time.
    fn activate_outgoing_pending(&self) -> bool;
    /// Whether requests from any source are taken, not only from testers.
    fn accept_all_requests(&self) -> bool;
    /// Whether a functional request is copied into a message of this ECU's own first.
    fn copy_functional_requests(&self) -> bool;
    /// The context the dispatcher runs on.
    fn context(&self) -> ContextType;
    /// The transport addresses.
    fn transport(&self) -> &TransportConfiguration;
    /// The outgoing connections the C++ configuration declares, for the shutdown log.
    fn outgoing_connection_count(&self) -> usize;
    /// Take a free connection, if any.
    fn acquire_incoming_diag_connection(&self) -> Option<&'static IncomingDiagConnection>;
    /// Give `connection` back.
    fn release_incoming_diag_connection(&self, connection: &IncomingDiagConnection);
    /// The connection whose address is `listener`'s, if it is one of the pool's.
    fn find_incoming_diag_connection(
        &self,
        listener: &dyn TransportMessageProcessedListener,
    ) -> Option<&'static IncomingDiagConnection>;
    /// Give every connection back.
    fn clear_incoming_diag_connections(&self);
    /// How many connections are in use.
    fn incoming_connections_in_use(&self) -> usize;
    /// How many connections there are.
    fn incoming_connection_count(&self) -> usize;
    /// Whether a job can be queued.
    fn send_job_queue_is_full(&self) -> bool;
    /// Whether no job is queued.
    fn send_job_queue_is_empty(&self) -> bool;
    /// Queue `job`; returns whether there was room.
    fn push_send_job(&self, job: TransportJob) -> bool;
    /// Take the next job.
    fn pop_send_job(&self) -> Option<TransportJob>;
}

/// The port of `declare::DiagnosisConfiguration<INCOMING, OUTGOING, QUEUE>`: `INCOMING`
/// connections and a queue of `QUEUE` jobs; outgoing connections are not ported, only
/// counted.
pub struct DiagnosisConfiguration<const INCOMING: usize, const OUTGOING: usize, const QUEUE: usize>
{
    diag_address: u16,
    broadcast_address: u16,
    diag_bus_id: u8,
    max_response_payload_size: u16,
    activate_outgoing_pending: bool,
    accept_all_requests: bool,
    copy_functional_requests: bool,
    context: ContextType,
    transport: TransportConfiguration,
    connections: [IncomingDiagConnection; INCOMING],
    send_job_queue: TransportJobQueue<QUEUE>,
}

impl<const INCOMING: usize, const OUTGOING: usize, const QUEUE: usize>
    DiagnosisConfiguration<INCOMING, OUTGOING, QUEUE>
{
    /// The C++ constructor's parameters, plus the transport addresses.
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub const fn new(
        diag_address: u16,
        broadcast_address: u16,
        diag_bus_id: u8,
        max_response_payload_size: u16,
        activate_outgoing_pending: bool,
        accept_all_requests: bool,
        copy_functional_requests: bool,
        context: ContextType,
        transport: TransportConfiguration,
    ) -> Self {
        Self {
            diag_address,
            broadcast_address,
            diag_bus_id,
            max_response_payload_size,
            activate_outgoing_pending,
            accept_all_requests,
            copy_functional_requests,
            context,
            transport,
            connections: [const { IncomingDiagConnection::new() }; INCOMING],
            send_job_queue: TransportJobQueue::new(),
        }
    }

    /// The connections, for tests.
    pub fn connections(&self) -> &[IncomingDiagConnection; INCOMING] {
        &self.connections
    }
}

impl<const INCOMING: usize, const OUTGOING: usize, const QUEUE: usize> DiagnosisConfig
    for DiagnosisConfiguration<INCOMING, OUTGOING, QUEUE>
{
    fn diag_address(&self) -> u16 {
        self.diag_address
    }

    fn broadcast_address(&self) -> u16 {
        self.broadcast_address
    }

    fn diag_bus_id(&self) -> u8 {
        self.diag_bus_id
    }

    fn max_response_payload_size(&self) -> u16 {
        self.max_response_payload_size
    }

    fn activate_outgoing_pending(&self) -> bool {
        self.activate_outgoing_pending
    }

    fn accept_all_requests(&self) -> bool {
        self.accept_all_requests
    }

    fn copy_functional_requests(&self) -> bool {
        self.copy_functional_requests
    }

    fn context(&self) -> ContextType {
        self.context
    }

    fn transport(&self) -> &TransportConfiguration {
        &self.transport
    }

    fn outgoing_connection_count(&self) -> usize {
        OUTGOING
    }

    fn acquire_incoming_diag_connection(&self) -> Option<&'static IncomingDiagConnection> {
        let connection = self.connections.iter().find(|connection| !connection.is_in_use())?;
        connection.set_in_use(true);
        connection.set_context(self.context);
        // SAFETY: a configuration is a `static` of the UDS system, so its connections live
        // forever.
        Some(unsafe { &*(connection as *const IncomingDiagConnection) })
    }

    fn release_incoming_diag_connection(&self, connection: &IncomingDiagConnection) {
        connection.set_in_use(false);
    }

    fn find_incoming_diag_connection(
        &self,
        listener: &dyn TransportMessageProcessedListener,
    ) -> Option<&'static IncomingDiagConnection> {
        let connection = self.connections.iter().find(|connection| {
            connection.is_in_use() && core::ptr::addr_eq(*connection, listener)
        })?;
        // SAFETY: as in `acquire_incoming_diag_connection`.
        Some(unsafe { &*(connection as *const IncomingDiagConnection) })
    }

    fn clear_incoming_diag_connections(&self) {
        for connection in &self.connections {
            connection.set_in_use(false);
        }
    }

    fn incoming_connections_in_use(&self) -> usize {
        self.connections.iter().filter(|connection| connection.is_in_use()).count()
    }

    fn incoming_connection_count(&self) -> usize {
        INCOMING
    }

    fn send_job_queue_is_full(&self) -> bool {
        self.send_job_queue.is_full()
    }

    fn send_job_queue_is_empty(&self) -> bool {
        self.send_job_queue.is_empty()
    }

    fn push_send_job(&self, job: TransportJob) -> bool {
        self.send_job_queue.push(job)
    }

    fn pop_send_job(&self) -> Option<TransportJob> {
        self.send_job_queue.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_configuration_knows_testers_and_the_functional_address() {
        let cut = TransportConfiguration::DEMO;
        assert!(cut.is_functional_address(0xDF));
        assert!(!cut.is_functional_address(0x2A));
        assert!(cut.is_tester_address(0xF0));
        assert!(cut.is_tester_address(0xFD));
        assert!(!cut.is_tester_address(0xFE));
        assert!(cut.is_tester_address(0xEF5));
        assert!(!cut.is_tester_address(0x2A));
    }

    #[test]
    fn the_job_queue_is_a_ring() {
        static MESSAGE: TransportMessage = TransportMessage::new();
        static LISTENER: openbsw_transport::DefaultTransportMessageProcessedListener =
            openbsw_transport::DefaultTransportMessageProcessedListener;
        let cut = TransportJobQueue::<2>::new();
        let job = TransportJob { message: &MESSAGE, processed_listener: &LISTENER };
        assert!(cut.is_empty());
        assert!(cut.push(job));
        assert!(cut.push(job));
        assert!(!cut.push(job));
        assert!(cut.is_full());
        assert!(cut.pop().is_some());
        assert!(cut.push(job));
        assert!(cut.pop().is_some());
        assert!(cut.pop().is_some());
        assert!(cut.pop().is_none());
    }
}
