//! Glue layer between the standalone `emthin-clipboard` crate and emthin's
//! smithay-based compositor state.
//!
//! - Maps [`SelectionKind`] ↔ smithay's [`SelectionTarget`].
//! - Routes [`ClipboardEvent`]s into the correct smithay selection API.
//! - Registers X11's async pipe drain (the only backend that emits
//!   [`AsyncCompletion`]) with calloop.
//!
//! All smithay-aware clipboard logic lives here so `emthin-clipboard` itself
//! stays free of smithay / calloop dependencies.

use emthin_clipboard::{AsyncCompletion, ClipboardEvent, SelectionKind};
use smithay::wayland::selection::SelectionTarget;

use crate::state::{EmthinState, SelectionOrigin};

/// Cross-crate conversion between smithay's [`SelectionTarget`] and
/// emthin-clipboard's [`SelectionKind`].
///
/// Orphan rules forbid `impl From<SelectionKind> for SelectionTarget`
/// (both types are foreign to this crate), so we expose the conversion
/// as extension traits whose trait type is local to emthin.
pub trait SelectionTargetExt {
    fn to_kind(self) -> SelectionKind;
}

pub trait SelectionKindExt {
    fn to_target(self) -> SelectionTarget;
}

impl SelectionTargetExt for SelectionTarget {
    fn to_kind(self) -> SelectionKind {
        match self {
            SelectionTarget::Clipboard => SelectionKind::Clipboard,
            SelectionTarget::Primary => SelectionKind::Primary,
        }
    }
}

impl SelectionKindExt for SelectionKind {
    fn to_target(self) -> SelectionTarget {
        match self {
            SelectionKind::Clipboard => SelectionTarget::Clipboard,
            SelectionKind::Primary => SelectionTarget::Primary,
        }
    }
}

pub fn handle_clipboard_event(state: &mut EmthinState, event: ClipboardEvent) {
    match event {
        ClipboardEvent::HostSelectionChanged { kind, mime_types } => {
            inject_host_selection(state, kind.to_target(), mime_types);
        }
        ClipboardEvent::HostSendRequest {
            kind,
            mime_type,
            write_fd,
            completion,
        } => {
            forward_client_selection(state, kind.to_target(), mime_type, write_fd);
            // Flush immediately so the write_fd reaches the Wayland client
            // before our OwnedFd copy is dropped (closing the write end).
            let _ = state.display_handle.flush_clients();
            if let Some(AsyncCompletion { id, read_fd }) = completion {
                if !register_outgoing_pipe(state, id, read_fd) {
                    // Calloop registration failed — clean up and notify X11 requestor.
                    if let Some(ref mut cb) = state.selection.clipboard {
                        cb.complete_outgoing(id, Vec::new());
                    }
                }
            }
        }
        ClipboardEvent::SourceCancelled { kind } => {
            let target = kind.to_target();
            tracing::debug!("Host source cancelled ({target:?})");
            match target {
                SelectionTarget::Clipboard => {
                    state.selection.clipboard_origin = SelectionOrigin::default();
                }
                SelectionTarget::Primary => {
                    state.selection.primary_origin = SelectionOrigin::default();
                }
            }
        }
    }
}

fn inject_host_selection(
    state: &mut EmthinState,
    target: SelectionTarget,
    mime_types: Vec<String>,
) {
    use smithay::wayland::selection::data_device::{
        clear_data_device_selection, set_data_device_selection,
    };
    use smithay::wayland::selection::primary_selection::{
        clear_primary_selection, set_primary_selection,
    };

    if mime_types.is_empty() {
        tracing::debug!("Host {target:?} cleared");
        match target {
            SelectionTarget::Clipboard => {
                clear_data_device_selection(&state.display_handle, &state.seat);
                state.selection.clipboard_origin = SelectionOrigin::default();
            }
            SelectionTarget::Primary => {
                clear_primary_selection(&state.display_handle, &state.seat);
                state.selection.primary_origin = SelectionOrigin::default();
            }
        }
    } else {
        tracing::debug!("Host {target:?} changed ({} types)", mime_types.len());
        match target {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&state.display_handle, &state.seat, mime_types, ());
                state.selection.clipboard_origin = SelectionOrigin::Host;
                state.selection.clipboard_cache = None;
            }
            SelectionTarget::Primary => {
                set_primary_selection(&state.display_handle, &state.seat, mime_types, ());
                state.selection.primary_origin = SelectionOrigin::Host;
            }
        }
    }
}

fn forward_client_selection(
    state: &mut EmthinState,
    target: SelectionTarget,
    mime_type: String,
    fd: std::os::fd::OwnedFd,
) {
    // Compositor-owned clipboard cache (set by M-w copy).
    // Check before routing by origin — same pattern as send_selection.
    if target == SelectionTarget::Clipboard {
        if let Some((ref mime_types, ref data)) = state.selection.clipboard_cache {
            if mime_types.iter().any(|m| m == &mime_type) {
                use std::io::Write;
                use std::os::fd::IntoRawFd;
                use std::os::unix::io::FromRawFd;
                let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
                let _ = file.write_all(data);
                tracing::debug!(
                    "forward_client_selection: wrote {} cached bytes for {mime_type}",
                    data.len()
                );
                return;
            }
        }
    }

    use smithay::wayland::selection::data_device::request_data_device_client_selection;
    use smithay::wayland::selection::primary_selection::request_primary_client_selection;

    let origin = match target {
        SelectionTarget::Clipboard => state.selection.clipboard_origin,
        SelectionTarget::Primary => state.selection.primary_origin,
    };

    tracing::debug!(
        "forward_client_selection: target={target:?} mime_type={mime_type} origin={origin:?}"
    );

    match origin {
        SelectionOrigin::Wayland => {
            let result = match target {
                SelectionTarget::Clipboard => {
                    request_data_device_client_selection(&state.seat, mime_type, fd)
                        .map_err(|e| format!("{e:?}"))
                }
                SelectionTarget::Primary => {
                    request_primary_client_selection(&state.seat, mime_type, fd)
                        .map_err(|e| format!("{e:?}"))
                }
            };
            if let Err(e) = result {
                tracing::warn!("Failed to forward {target:?} selection to host: {e}");
            } else {
                tracing::debug!("forward_client_selection: {target:?} -> host ok");
            }
        }
        SelectionOrigin::Host => {
            // Host asked us for data we got from them — shouldn't
            // happen in practice (host has the data natively). Drop
            // the fd so the peer gets EOF instead of hanging.
            tracing::debug!(
                "Ignoring {target:?} selection forward: origin is host (no local source)"
            );
            drop(fd);
        }
    }
}

/// Register a pipe read_fd with calloop for event-driven reading.
/// Returns `false` if registration fails (caller should clean up).
fn register_outgoing_pipe(state: &mut EmthinState, id: u64, read_fd: std::os::fd::OwnedFd) -> bool {
    use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
    use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};

    // SAFETY: into_raw_fd() relinquishes ownership; File takes it over.
    let file = unsafe { std::fs::File::from_raw_fd(read_fd.into_raw_fd()) };
    let mut buf_data: Vec<u8> = Vec::new();

    if let Err(e) = state.loop_handle.insert_source(
        Generic::new(file, Interest::READ, Mode::Level),
        move |_, file, state| {
            let mut buf = [0u8; 65536];
            loop {
                // SAFETY: buf is valid for buf.len() bytes; fd is open and non-blocking.
                let n = unsafe { libc::read(file.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n > 0 {
                    buf_data.extend_from_slice(&buf[..n as usize]);
                } else if n == 0 {
                    let data = std::mem::take(&mut buf_data);
                    if let Some(ref mut clipboard) = state.selection.clipboard {
                        clipboard.complete_outgoing(id, data);
                    }
                    return Ok(PostAction::Remove);
                } else {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::WouldBlock {
                        return Ok(PostAction::Continue);
                    }
                    tracing::warn!("outgoing pipe read error: {err}");
                    // Hand whatever we drained so far to the backend so its
                    // outgoing_requests entry is retired and the peer gets
                    // a SelectionNotify (truncated data or failure) rather
                    // than hanging forever.
                    let data = std::mem::take(&mut buf_data);
                    if let Some(ref mut clipboard) = state.selection.clipboard {
                        clipboard.complete_outgoing(id, data);
                    }
                    return Ok(PostAction::Remove);
                }
            }
        },
    ) {
        tracing::warn!("Failed to register outgoing pipe: {e}");
        return false;
    }
    true
}
