use gio::prelude::ToVariant;
use gio::DBusMessage;

pub const INPUT_METHOD_INTERFACE: &str = "org.fcitx.Fcitx.InputMethod1";
pub const INPUT_CONTEXT_INTERFACE: &str = "org.fcitx.Fcitx.InputContext1";
pub const INPUT_CONTEXT_INTERFACE_FCITX4: &str = "org.fcitx.Fcitx.InputContext";

pub const FCITX5_WELL_KNOWN_NAMES: &[&str] = &[
    "org.fcitx.Fcitx5",
    "org.freedesktop.portal.Fcitx",
    "org.fcitx.Fcitx",
];

pub const INPUT_CONTEXT_PATH_PREFIX: &str = "/org/freedesktop/portal/inputcontext/";

pub fn is_fcitx_interface(iface: &str) -> bool {
    matches!(
        iface,
        INPUT_METHOD_INTERFACE | INPUT_CONTEXT_INTERFACE | INPUT_CONTEXT_INTERFACE_FCITX4
    )
}

pub fn is_fcitx_well_known(name: &str) -> bool {
    FCITX5_WELL_KNOWN_NAMES.contains(&name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FcitxEvent {
    FocusChanged { ic_path: String, focused: bool },
    CursorRect { ic_path: String, rect: [i32; 4] },
    IcDestroyed { ic_path: String },
}

pub fn method_call_to_event(method: &Fcitx5MethodCall) -> Option<FcitxEvent> {
    match method {
        Fcitx5MethodCall::FocusIn { input_context_path } => Some(FcitxEvent::FocusChanged {
            ic_path: input_context_path.clone(),
            focused: true,
        }),
        Fcitx5MethodCall::FocusOut { input_context_path } => Some(FcitxEvent::FocusChanged {
            ic_path: input_context_path.clone(),
            focused: false,
        }),
        Fcitx5MethodCall::SetCursorRect {
            input_context_path,
            x,
            y,
            w,
            h,
        } => Some(FcitxEvent::CursorRect {
            ic_path: input_context_path.clone(),
            rect: [*x, *y, *w, *h],
        }),
        Fcitx5MethodCall::SetCursorRectV2 {
            input_context_path,
            x,
            y,
            w,
            h,
            scale,
        } => {
            let s = if *scale > 0.0 { *scale } else { 1.0 };
            let tl = |v: i32| (v as f64 / s).round() as i32;
            Some(FcitxEvent::CursorRect {
                ic_path: input_context_path.clone(),
                rect: [tl(*x), tl(*y), tl(*w), tl(*h)],
            })
        }
        Fcitx5MethodCall::SetCursorLocation {
            input_context_path,
            x,
            y,
        } => Some(FcitxEvent::CursorRect {
            ic_path: input_context_path.clone(),
            rect: [*x, *y, 0, 0],
        }),
        Fcitx5MethodCall::DestroyIC { input_context_path } => Some(FcitxEvent::IcDestroyed {
            ic_path: input_context_path.clone(),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fcitx5MethodCall {
    CreateInputContext {
        hints: Vec<(String, String)>,
    },
    FocusIn {
        input_context_path: String,
    },
    FocusOut {
        input_context_path: String,
    },
    Reset {
        input_context_path: String,
    },
    DestroyIC {
        input_context_path: String,
    },
    SetCapability {
        input_context_path: String,
        capability: u64,
    },
    SetCursorRect {
        input_context_path: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    SetCursorRectV2 {
        input_context_path: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        scale: f64,
    },
    SetCursorLocation {
        input_context_path: String,
        x: i32,
        y: i32,
    },
    SetSurroundingText {
        input_context_path: String,
        text: String,
        cursor: u32,
        anchor: u32,
    },
    SetSurroundingTextPosition {
        input_context_path: String,
        cursor: u32,
        anchor: u32,
    },
}

pub fn classify(msg: &DBusMessage) -> Option<Fcitx5MethodCall> {
    let iface = msg.interface()?;
    let member = msg.member()?;
    let path = msg.path()?;
    let body = msg.body();

    match (iface.as_str(), member.as_str()) {
        (INPUT_METHOD_INTERFACE, "CreateInputContext") => {
            let hints: Vec<(String, String)> = body?.child_value(0).get()?;
            Some(Fcitx5MethodCall::CreateInputContext { hints })
        }
        (INPUT_CONTEXT_INTERFACE, "FocusIn") => Some(Fcitx5MethodCall::FocusIn {
            input_context_path: path.to_string(),
        }),
        (INPUT_CONTEXT_INTERFACE, "FocusOut") => Some(Fcitx5MethodCall::FocusOut {
            input_context_path: path.to_string(),
        }),
        (INPUT_CONTEXT_INTERFACE, "Reset") => Some(Fcitx5MethodCall::Reset {
            input_context_path: path.to_string(),
        }),
        (INPUT_CONTEXT_INTERFACE, "DestroyIC") => Some(Fcitx5MethodCall::DestroyIC {
            input_context_path: path.to_string(),
        }),
        (INPUT_CONTEXT_INTERFACE, "SetCapability") => {
            let capability: u64 = body?.child_value(0).get()?;
            Some(Fcitx5MethodCall::SetCapability {
                input_context_path: path.to_string(),
                capability,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetCursorRect") => {
            let (x, y, w, h): (i32, i32, i32, i32) = body?.get()?;
            Some(Fcitx5MethodCall::SetCursorRect {
                input_context_path: path.to_string(),
                x,
                y,
                w,
                h,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetCursorRectV2") => {
            let (x, y, w, h, scale): (i32, i32, i32, i32, f64) = body?.get()?;
            Some(Fcitx5MethodCall::SetCursorRectV2 {
                input_context_path: path.to_string(),
                x,
                y,
                w,
                h,
                scale,
            })
        }
        (INPUT_CONTEXT_INTERFACE_FCITX4, "SetCursorLocation") => {
            let (x, y): (i32, i32) = body?.get()?;
            Some(Fcitx5MethodCall::SetCursorLocation {
                input_context_path: path.to_string(),
                x,
                y,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetSurroundingText") => {
            let (text, cursor, anchor): (String, u32, u32) = body?.get()?;
            let ic_path = msg.path()?.to_string();
            Some(Fcitx5MethodCall::SetSurroundingText {
                input_context_path: ic_path,
                text,
                cursor,
                anchor,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetSurroundingTextPosition") => {
            let (cursor, anchor): (u32, u32) = body?.get()?;
            Some(Fcitx5MethodCall::SetSurroundingTextPosition {
                input_context_path: path.to_string(),
                cursor,
                anchor,
            })
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct InputContextAllocator {
    next_id: u64,
}

impl InputContextAllocator {
    pub fn new() -> Self {
        Self { next_id: 1 }
    }

    pub fn allocate(&mut self) -> (String, [u8; 16]) {
        let id = self.next_id;
        self.next_id += 1;
        let path = format!("{INPUT_CONTEXT_PATH_PREFIX}{id}");
        let mut uuid = [0u8; 16];
        uuid[..8].copy_from_slice(&id.to_le_bytes());
        (path, uuid)
    }

    pub fn peek_next(&self) -> u64 {
        self.next_id
    }
}

pub fn build_reply(
    request: &DBusMessage,
    method: &Fcitx5MethodCall,
    ic_alloc: &mut InputContextAllocator,
) -> Option<DBusMessage> {
    let reply = request.new_method_reply();
    if let Fcitx5MethodCall::CreateInputContext { .. } = method {
        let (path, uuid) = ic_alloc.allocate();
        let obj_path = glib::variant::ObjectPath::try_from(path.as_str()).ok()?;
        let body = (obj_path, uuid.to_vec()).to_variant();
        reply.set_body(&body);
    }
    Some(reply)
}

pub fn build_preedit_chunks(
    text: &str,
    cursor: Option<(i32, i32)>,
    underline: i32,
    highlight: i32,
) -> Vec<(i32, i32, String)> {
    use unicode_segmentation::UnicodeSegmentation;
    let grapheme_indices: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    if grapheme_indices.is_empty() {
        return vec![];
    }
    if let Some((begin, end)) = cursor {
        let begin = begin.max(0) as usize;
        let end = end.max(begin as i32) as usize;
        let mut chunks: Vec<(i32, i32, String)> = Vec::new();
        let mut pos = 0usize;
        if pos < begin.min(grapheme_indices.len()) {
            let _end_pos = grapheme_indices[begin.min(grapheme_indices.len())].0;
            let segment: String = grapheme_indices[pos..begin.min(grapheme_indices.len())]
                .iter()
                .map(|(_, g)| *g)
                .collect();
            if !segment.is_empty() {
                chunks.push((underline, 0, segment));
            }
            pos = begin.min(grapheme_indices.len());
        }
        if pos < end.min(grapheme_indices.len()) {
            let _end_pos = grapheme_indices[end.min(grapheme_indices.len())].0;
            let segment: String = grapheme_indices[pos..end.min(grapheme_indices.len())]
                .iter()
                .map(|(_, g)| *g)
                .collect();
            if !segment.is_empty() {
                chunks.push((underline | highlight, 0, segment));
            }
            pos = end.min(grapheme_indices.len());
        }
        if pos < grapheme_indices.len() {
            let segment: String = grapheme_indices[pos..].iter().map(|(_, g)| *g).collect();
            if !segment.is_empty() {
                chunks.push((underline, 0, segment));
            }
        }
        chunks
    } else {
        let text = text.to_string();
        if text.is_empty() {
            vec![]
        } else {
            vec![(underline, 0, text)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(iface: &str, member: &str, path: &str) -> DBusMessage {
        DBusMessage::new_method_call(Some("org.fcitx.Fcitx5"), path, Some(iface), member)
    }

    fn make_call_body<T: super::ToVariant>(
        iface: &str,
        member: &str,
        path: &str,
        body: &T,
    ) -> DBusMessage {
        let msg = make_call(iface, member, path);
        let v = body.to_variant();
        // GLib requires the body to be a tuple variant. Single-argument
        // bodies must be wrapped in a tuple.
        let tuple = match v.type_().to_string().as_str() {
            s if s.starts_with('(') => v, // already a tuple
            _ => glib::Variant::tuple_from_iter([&v]),
        };
        msg.set_body(&tuple);
        msg
    }

    fn ic_request(member: &str, path: &str, serial: u32) -> DBusMessage {
        let msg = make_call(INPUT_CONTEXT_INTERFACE, member, path);
        msg.set_serial(serial);
        msg.set_sender(Some(":1.42"));
        msg
    }

    fn create_input_context_request(serial: u32) -> DBusMessage {
        let hints: Vec<(String, String)> = Vec::new();
        let msg = make_call_body(INPUT_METHOD_INTERFACE, "CreateInputContext", "/im", &hints);
        msg.set_serial(serial);
        msg.set_sender(Some(":1.42"));
        msg
    }

    #[test]
    fn is_fcitx_interface_matches_known_ifaces() {
        assert!(is_fcitx_interface("org.fcitx.Fcitx.InputMethod1"));
        assert!(is_fcitx_interface("org.fcitx.Fcitx.InputContext1"));
        assert!(is_fcitx_interface("org.fcitx.Fcitx.InputContext"));
    }

    #[test]
    fn is_fcitx_interface_rejects_unrelated() {
        assert!(!is_fcitx_interface("org.freedesktop.DBus"));
        assert!(!is_fcitx_interface("org.fcitx.Fcitx.InputMethod"));
        assert!(!is_fcitx_interface(""));
    }

    #[test]
    fn classifies_focus_in() {
        let msg = make_call(INPUT_CONTEXT_INTERFACE, "FocusIn", "/ic/7");
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::FocusIn {
                input_context_path: "/ic/7".into()
            })
        );
    }

    #[test]
    fn classifies_focus_out() {
        let msg = make_call(INPUT_CONTEXT_INTERFACE, "FocusOut", "/ic/1");
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::FocusOut {
                input_context_path: "/ic/1".into()
            })
        );
    }

    #[test]
    fn classifies_reset() {
        let msg = make_call(INPUT_CONTEXT_INTERFACE, "Reset", "/ic/1");
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::Reset {
                input_context_path: "/ic/1".into()
            })
        );
    }

    #[test]
    fn classifies_destroy_ic() {
        let msg = make_call(INPUT_CONTEXT_INTERFACE, "DestroyIC", "/ic/2");
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::DestroyIC {
                input_context_path: "/ic/2".into()
            })
        );
    }

    #[test]
    fn classifies_set_cursor_rect() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE,
            "SetCursorRect",
            "/ic/7",
            &(100i32, 200i32, 10i32, 20i32),
        );
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::SetCursorRect {
                input_context_path: "/ic/7".into(),
                x: 100,
                y: 200,
                w: 10,
                h: 20,
            })
        );
    }

    #[test]
    fn classifies_set_cursor_rect_v2() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE,
            "SetCursorRectV2",
            "/ic/7",
            &(10i32, 20i32, 30i32, 40i32, 1.25f64),
        );
        let Some(Fcitx5MethodCall::SetCursorRectV2 {
            input_context_path,
            x,
            y,
            w,
            h,
            scale,
        }) = classify(&msg)
        else {
            panic!("not V2");
        };
        assert_eq!(input_context_path, "/ic/7");
        assert_eq!((x, y, w, h), (10, 20, 30, 40));
        assert_eq!(scale, 1.25);
    }

    #[test]
    fn classifies_fcitx4_set_cursor_location() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE_FCITX4,
            "SetCursorLocation",
            "/ic/7",
            &(50i32, 60i32),
        );
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::SetCursorLocation {
                input_context_path: "/ic/7".into(),
                x: 50,
                y: 60,
            })
        );
    }

    #[test]
    fn process_key_event_is_not_classified() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE,
            "ProcessKeyEvent",
            "/ic/7",
            &(0x61u32, 38u32, 0u32, false, 1234u32),
        );
        assert_eq!(classify(&msg), None);
    }

    #[test]
    fn classifies_set_capability() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE,
            "SetCapability",
            "/ic/7",
            &0xDEADBEEFu64,
        );
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::SetCapability {
                input_context_path: "/ic/7".into(),
                capability: 0xDEADBEEF,
            })
        );
    }

    #[test]
    fn classifies_create_input_context_empty() {
        let hints: Vec<(String, String)> = Vec::new();
        let msg = make_call_body(INPUT_METHOD_INTERFACE, "CreateInputContext", "/im", &hints);
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::CreateInputContext { hints: vec![] })
        );
    }

    #[test]
    fn classifies_create_input_context_with_one_hint() {
        let hints: Vec<(String, String)> = vec![("program".into(), "wechat".into())];
        let msg = make_call_body(INPUT_METHOD_INTERFACE, "CreateInputContext", "/im", &hints);
        assert_eq!(
            classify(&msg),
            Some(Fcitx5MethodCall::CreateInputContext {
                hints: vec![("program".into(), "wechat".into())],
            })
        );
    }

    #[test]
    fn unrelated_interface_is_not_classified() {
        let msg = make_call("org.freedesktop.DBus", "Hello", "/org/freedesktop/DBus");
        assert_eq!(classify(&msg), None);
    }

    #[test]
    fn unknown_member_on_known_iface_is_not_classified() {
        let msg = make_call_body(
            INPUT_CONTEXT_INTERFACE,
            "MysterySettings",
            "/ic/7",
            &(0i32, 0i32, 0i32),
        );
        assert_eq!(classify(&msg), None);
    }

    #[test]
    fn allocator_first_id_is_1() {
        let mut a = InputContextAllocator::new();
        let (path, _) = a.allocate();
        assert_eq!(path, "/org/freedesktop/portal/inputcontext/1");
    }

    #[test]
    fn allocator_ids_increase_monotonically() {
        let mut a = InputContextAllocator::new();
        let (p1, _) = a.allocate();
        let (p2, _) = a.allocate();
        let (p3, _) = a.allocate();
        assert_ne!(p1, p2);
        assert_ne!(p2, p3);
        assert!(p3.ends_with("/3"));
    }

    #[test]
    fn allocator_uuid_encodes_id_in_low_bytes() {
        let mut a = InputContextAllocator::new();
        let (_, uuid) = a.allocate();
        assert_eq!(&uuid[..8], &1u64.to_le_bytes());
        assert_eq!(&uuid[8..], &[0u8; 8]);
    }

    #[test]
    fn create_input_context_returns_oay_with_swapped_endpoints() {
        let msg = create_input_context_request(42);
        let mut alloc = InputContextAllocator::new();
        let reply = build_reply(
            &msg,
            &Fcitx5MethodCall::CreateInputContext { hints: vec![] },
            &mut alloc,
        )
        .unwrap();
        assert_eq!(reply.message_type(), gio::DBusMessageType::MethodReturn);
        assert_eq!(reply.reply_serial(), 42);
        assert_eq!(reply.destination().as_deref(), Some(":1.42"));
        assert_eq!(reply.signature().as_str(), "oay");
    }

    #[test]
    fn create_input_context_allocates_fresh_path_each_call() {
        let mut alloc = InputContextAllocator::new();
        for serial_n in 1u32..=3 {
            let msg = create_input_context_request(serial_n);
            build_reply(
                &msg,
                &Fcitx5MethodCall::CreateInputContext { hints: vec![] },
                &mut alloc,
            );
        }
        let (path, _) = alloc.allocate();
        assert!(path.ends_with("/4"), "got {path}");
    }

    #[test]
    fn focus_in_returns_empty_method_return() {
        let msg = ic_request("FocusIn", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply = build_reply(
            &msg,
            &Fcitx5MethodCall::FocusIn {
                input_context_path: "/ic/7".into(),
            },
            &mut alloc,
        )
        .unwrap();
        assert_eq!(reply.message_type(), gio::DBusMessageType::MethodReturn);
        assert_eq!(reply.body().map(|b| b.n_children()).unwrap_or(0), 0);
        assert_eq!(reply.reply_serial(), 1);
    }

    #[test]
    fn destroy_ic_returns_empty_method_return() {
        let msg = ic_request("DestroyIC", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply = build_reply(
            &msg,
            &Fcitx5MethodCall::DestroyIC {
                input_context_path: "/ic/7".into(),
            },
            &mut alloc,
        )
        .unwrap();
        assert_eq!(reply.message_type(), gio::DBusMessageType::MethodReturn);
        assert_eq!(reply.body().map(|b| b.n_children()).unwrap_or(0), 0);
    }

    #[test]
    fn set_cursor_rect_v2_returns_empty_method_return() {
        let msg = ic_request("SetCursorRectV2", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply = build_reply(
            &msg,
            &Fcitx5MethodCall::SetCursorRectV2 {
                input_context_path: "/ic/7".into(),
                x: 100,
                y: 200,
                w: 10,
                h: 20,
                scale: 1.0,
            },
            &mut alloc,
        )
        .unwrap();
        assert_eq!(reply.body().map(|b| b.n_children()).unwrap_or(0), 0);
    }
}
