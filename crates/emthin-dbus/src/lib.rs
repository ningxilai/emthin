//! emthin-dbus — DBus session-bus protocol primitives for nested Wayland
//! compositors.
//!
//! Modules:
//!   - [`wire`] — DBus v1 wire format (SASL handshake scanner +
//!     [`wire::frame::Frame`] parser/encoder, both built on `zvariant`).
//!   - [`broker`] — per-connection byte-stream state machine that
//!     consumes raw socket bytes and reports complete frames.
//!   - [`fcitx`] — fcitx5 frontend recognizer: classify intercepted
//!     method_calls, allocate input contexts, synthesize replies.
//!     Also holds the compositor-facing [`FcitxEvent`] type.
//!   - [`proxy`] — in-process broker IO loop: listener, upstream
//!     dialing, per-connection `recvmsg`/`sendmsg` pumps with
//!     `SCM_RIGHTS` fd passing, and the synthesized fcitx5
//!     `CommitString` / `UpdateFormattedPreedit` signal emitters. Pure
//!     enough that it tests with plain `socketpair()` — no calloop or
//!     smithay dep — but the consumer crate is responsible for
//!     wiring the broker's fds into its own event loop.
//!   - [`router`] — routing rule types + IPC protocol messages for the
//!     standalone `emthin-dbus-router` subprocess.
//!
//! Common types are re-exported at the crate root for ergonomic use.

pub mod broker;
pub mod fcitx;
pub mod proxy;
pub mod router;
pub mod wire;

// Re-exports — lets downstream write `emthin_dbus::Frame` instead of
// drilling through `emthin_dbus::wire::frame::Frame`.
pub use broker::state::{BrokerError, ConnectionState, FeedOutcome};
pub use fcitx::{
    build_reply, classify, method_call_to_event, Fcitx5MethodCall, InputContextAllocator,
};
pub use proxy::{
    parse_unix_bus_address, ConnAccepted, ConnId, DbusBroker, FcitxEvent, PumpOutcome,
};
pub use router::{RouteRule, RouterNotification, RouterRequest};
pub use wire::frame::{
    BodyBuilder, FieldCode, Frame, FrameBuilder, FrameError, Headers, MessageKind, SerialCounter,
};
