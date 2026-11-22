//! Session tuning.

use std::time::Duration;

use crate::frame::{MAX_SEGMENT, MAX_STREAM_WINDOW};

/// Knobs for a [`Session`](crate::Session).
///
/// The Go original exposes 20 fields, many of which only exist to switch off a
/// heuristic (`DisableAdaptivePingTimeout`, `DisablePingPadRandom`,
/// `DisableTrafficAwarePing`, ...). Those switches are omitted here: a knob that
/// only ever turns off a feature is a sign the feature should not have been
/// unconditional, and each one doubles the states worth testing.
#[derive(Debug, Clone)]
pub struct Config {
    /// Receive credit advertised when a stream opens.
    pub initial_stream_window: u32,
    /// Ceiling for one stream's receive window as it grows.
    pub max_stream_window: u32,
    /// Ceiling for buffered receive data across every stream.
    ///
    /// Without a session-wide cap, N streams each entitled to
    /// `max_stream_window` could pin N times that much memory.
    pub max_session_window: u64,
    /// Minimum interval between liveness probes.
    pub keepalive_interval: Duration,
    /// Maximum unanswered-probe timeout when incoming frames stop progressing.
    ///
    /// Observed RTT scales the deadline between `keepalive_interval` and this
    /// ceiling. Valid traffic also proves liveness when a pong is delayed by
    /// a bulk transfer on the same ordered transport.
    pub keepalive_timeout: Duration,
    /// How long to wait for a peer to accept or refuse an open request.
    pub open_timeout: Duration,
    /// Streams one side may have open at once.
    pub max_streams: usize,
    /// How many accepted-but-not-yet-taken streams to hold before applying
    /// backpressure to the peer.
    pub accept_backlog: usize,
    /// Bytes retained in outbound DATA frames, including frame headers.
    pub max_send_queue: usize,
    /// Maximum queued control frames. Overflow closes the session.
    pub max_control_queue: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 16 segments: enough to keep a 100 Mbit/50 ms path full without
            // committing much memory per idle stream.
            initial_stream_window: 16 * MAX_SEGMENT as u32,
            max_stream_window: 8 * 1024 * 1024,
            max_session_window: 256 * 1024 * 1024,
            keepalive_interval: Duration::from_secs(15),
            keepalive_timeout: Duration::from_secs(30),
            open_timeout: Duration::from_secs(30),
            max_streams: 1024,
            accept_backlog: 256,
            max_send_queue: 8 * 1024 * 1024,
            max_control_queue: 8192,
        }
    }
}

impl Config {
    /// Clamps the fields into a coherent range.
    ///
    /// Called on construction so the session never has to re-check invariants at
    /// runtime. Every fix-up here is a value that would otherwise deadlock or
    /// stall the session rather than merely perform badly.
    pub fn normalized(mut self) -> Self {
        // A window under one segment could never admit a full-size frame.
        self.initial_stream_window = self
            .initial_stream_window
            .clamp(MAX_SEGMENT as u32, MAX_STREAM_WINDOW);
        self.max_stream_window = self
            .max_stream_window
            .clamp(self.initial_stream_window, MAX_STREAM_WINDOW);
        self.max_session_window = self.max_session_window.max(self.max_stream_window as u64);
        self.max_send_queue = self
            .max_send_queue
            .clamp(MAX_SEGMENT + crate::frame::HEADER_LEN, 256 * 1024 * 1024);
        self.max_control_queue = self.max_control_queue.clamp(16, 65536);
        self.keepalive_interval = self.keepalive_interval.max(Duration::from_millis(1));
        self.open_timeout = self.open_timeout.max(Duration::from_millis(1));

        // Waiting for a pong for less time than we wait between pings would
        // declare a healthy link dead.
        if self.keepalive_timeout <= self.keepalive_interval {
            self.keepalive_timeout = self.keepalive_interval * 2;
        }

        self.max_streams = self.max_streams.max(1);
        self.accept_backlog = self.accept_backlog.max(1);

        // Initial advertisements must fit even before adaptive growth. A
        // runtime reservation also bounds growth and refuses new opens when
        // existing streams have consumed the remaining receive budget.
        let per_stream = u64::from(self.initial_stream_window);
        let affordable = (self.max_session_window / per_stream).max(1);
        if (self.max_streams as u64) > affordable {
            self.max_streams = affordable as usize;
        }
        self
    }

    /// Bytes reserved initially if every stream slot is occupied.
    pub fn worst_case_buffered(&self) -> u64 {
        self.max_streams as u64 * u64::from(self.initial_stream_window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_already_coherent() {
        let d = Config::default();
        assert_eq!(format!("{:?}", d.clone().normalized()), format!("{d:?}"));
    }

    #[test]
    fn a_window_below_one_segment_is_raised() {
        let c = Config {
            initial_stream_window: 1,
            ..Default::default()
        }
        .normalized();
        assert!(c.initial_stream_window >= MAX_SEGMENT as u32);
    }

    #[test]
    fn window_ceilings_are_ordered() {
        let c = Config {
            initial_stream_window: 4 * 1024 * 1024,
            max_stream_window: 1024,
            max_session_window: 2048,
            ..Default::default()
        }
        .normalized();
        assert!(c.max_stream_window >= c.initial_stream_window);
        assert!(c.max_session_window >= c.max_stream_window as u64);
    }

    #[test]
    fn a_timeout_shorter_than_the_ping_interval_is_widened() {
        let c = Config {
            keepalive_interval: Duration::from_secs(30),
            keepalive_timeout: Duration::from_secs(5),
            ..Default::default()
        }
        .normalized();
        assert!(
            c.keepalive_timeout > c.keepalive_interval,
            "would kill a healthy link before its first pong could arrive"
        );
    }

    #[test]
    fn the_stream_count_is_capped_to_respect_the_session_window() {
        let c = Config {
            initial_stream_window: 1024 * 1024,
            max_session_window: 8 * 1024 * 1024,
            max_streams: 10_000,
            ..Default::default()
        }
        .normalized();
        assert_eq!(c.max_streams, 8, "8 streams x 1MiB is the whole budget");
        assert!(c.worst_case_buffered() <= c.max_session_window);
    }

    #[test]
    fn the_default_budget_is_within_its_own_ceiling() {
        let c = Config::default().normalized();
        assert!(
            c.worst_case_buffered() <= c.max_session_window,
            "{} > {}",
            c.worst_case_buffered(),
            c.max_session_window
        );
    }

    #[test]
    fn at_least_one_stream_survives_an_absurd_window() {
        let c = Config {
            initial_stream_window: u32::MAX,
            max_session_window: 1,
            ..Default::default()
        }
        .normalized();
        assert_eq!(c.max_streams, 1, "a session with no streams is useless");
    }

    #[test]
    fn zero_limits_are_raised_to_one() {
        let c = Config {
            max_streams: 0,
            accept_backlog: 0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(c.max_streams, 1);
        assert_eq!(c.accept_backlog, 1);
    }
}
