Source: crates.io `h3-quinn` 0.0.10, upstream <https://github.com/hyperium/h3>.
The original MIT license is retained in LICENSE.

`RecvStream::poll_data` previously moved the Quinn receive stream into a
reusable future and left `self.stream = None` while pending. Both
`stop_sending` and `recv_id` then unconditionally unwrapped that option.
Cancelling a pending HTTP/3 request body reproduced a panic in production code.

The adapter now owns its receive stream at all times and polls a borrowed,
stack-pinned `read_chunk` future. Quinn explicitly documents `read_chunk` as
cancellation-safe; pending read state and the wakeup remain in the stream.
This also removes the per-stream boxed future.

Felucca's tests cancel HTTP/3 streams while reads are pending and check that
other streams on the same connection keep working.
