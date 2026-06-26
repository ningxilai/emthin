use zbus::message::Message;
use zvariant::serialized::{Context, Data};

// ====================================================================
// FcitxEvent — typed events emitted to the compositor's ImeBridge.
// ====================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

// ====================================================================
// Preedit signal helpers
// ====================================================================

pub fn build_preedit_chunks(
    text: &str,
    cursor: Option<(i32, i32)>,
    underline: i32,
    highlight: i32,
) -> Vec<(String, i32)> {
    let plain = || vec![(text.to_string(), underline)];
    let Some((begin, end)) = cursor else {
        return plain();
    };
    if begin < 0 || end <= begin {
        return plain();
    }
    let (b, e) = (begin as usize, end as usize);
    let len = text.len();
    if b > len || e > len || !text.is_char_boundary(b) || !text.is_char_boundary(e) {
        return plain();
    }
    let mut v = Vec::with_capacity(3);
    if b > 0 {
        v.push((text[..b].to_string(), underline));
    }
    v.push((text[b..e].to_string(), underline | highlight));
    if e < len {
        v.push((text[e..].to_string(), underline));
    }
    v
}

// ====================================================================
// Constants
// ====================================================================

pub const INPUT_METHOD_INTERFACE: &str = "org.fcitx.Fcitx.InputMethod1";
pub const INPUT_CONTEXT_INTERFACE: &str = "org.fcitx.Fcitx.InputContext1";
pub const INPUT_CONTEXT_INTERFACE_FCITX4: &str = "org.fcitx.Fcitx.InputContext";

pub const FCITX5_WELL_KNOWN_NAMES: &[&str] = &[
    "org.fcitx.Fcitx5",
    "org.freedesktop.portal.Fcitx",
    "org.fcitx.Fcitx",
];

pub const INPUT_CONTEXT_PATH_PREFIX: &str = "/org/freedesktop/portal/inputcontext/";

// ====================================================================
// Recognition predicates
// ====================================================================

pub fn is_fcitx_interface(iface: &str) -> bool {
    matches!(
        iface,
        INPUT_METHOD_INTERFACE | INPUT_CONTEXT_INTERFACE | INPUT_CONTEXT_INTERFACE_FCITX4
    )
}

pub fn is_fcitx_well_known(name: &str) -> bool {
    FCITX5_WELL_KNOWN_NAMES.contains(&name)
}

// ====================================================================
// Typed fcitx5 method calls
// ====================================================================

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

// ====================================================================
// Classify
// ====================================================================

fn msg_from_bytes(bytes: &[u8]) -> Option<Message> {
    let ctx = Context::new_dbus(zvariant::Endian::Little, 0);
    let data = Data::new(bytes.to_vec(), ctx);
    unsafe { Message::from_bytes(data).ok() }
}

pub fn classify(bytes: &[u8]) -> Option<Fcitx5MethodCall> {
    let msg = msg_from_bytes(bytes)?;
    let header = msg.header();
    let iface = header.interface()?.as_str().to_owned();
    let member = header.member()?.as_str().to_owned();
    let sig = header.signature().to_string_no_parens();
    let path = header.path().map(|p| p.as_str().to_owned());
    let body = msg.body();
    let body_data = body.data();

    let sig_str = sig.as_str();

    match (iface.as_str(), member.as_str(), sig_str) {
        (INPUT_METHOD_INTERFACE, "CreateInputContext", "a(ss)") => {
            let hints = body_data
                .deserialize_for_signature::<&str, Vec<(String, String)>>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::CreateInputContext { hints })
        }
        (INPUT_CONTEXT_INTERFACE, "FocusIn", "") => Some(Fcitx5MethodCall::FocusIn {
            input_context_path: path?,
        }),
        (INPUT_CONTEXT_INTERFACE, "FocusOut", "") => Some(Fcitx5MethodCall::FocusOut {
            input_context_path: path?,
        }),
        (INPUT_CONTEXT_INTERFACE, "Reset", "") => Some(Fcitx5MethodCall::Reset {
            input_context_path: path?,
        }),
        (INPUT_CONTEXT_INTERFACE, "DestroyIC", "") => Some(Fcitx5MethodCall::DestroyIC {
            input_context_path: path?,
        }),
        (INPUT_CONTEXT_INTERFACE, "SetCapability", "t") => {
            let capability: u64 = body_data
                .deserialize_for_signature::<&str, u64>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetCapability {
                input_context_path: path?,
                capability,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetCursorRect", "iiii") => {
            let (x, y, w, h): (i32, i32, i32, i32) = body_data
                .deserialize_for_signature::<&str, (i32, i32, i32, i32)>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetCursorRect {
                input_context_path: path?,
                x,
                y,
                w,
                h,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetCursorRectV2", "iiiid") => {
            let (x, y, w, h, scale): (i32, i32, i32, i32, f64) = body_data
                .deserialize_for_signature::<&str, (i32, i32, i32, i32, f64)>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetCursorRectV2 {
                input_context_path: path?,
                x,
                y,
                w,
                h,
                scale,
            })
        }
        (INPUT_CONTEXT_INTERFACE_FCITX4, "SetCursorLocation", "ii") => {
            let (x, y): (i32, i32) = body_data
                .deserialize_for_signature::<&str, (i32, i32)>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetCursorLocation {
                input_context_path: path?,
                x,
                y,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetSurroundingText", "suu") => {
            let (text, cursor, anchor): (String, u32, u32) = body_data
                .deserialize_for_signature::<&str, (String, u32, u32)>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetSurroundingText {
                input_context_path: path?,
                text,
                cursor,
                anchor,
            })
        }
        (INPUT_CONTEXT_INTERFACE, "SetSurroundingTextPosition", "uu") => {
            let (cursor, anchor): (u32, u32) = body_data
                .deserialize_for_signature::<&str, (u32, u32)>(sig_str)
                .ok()?
                .0;
            Some(Fcitx5MethodCall::SetSurroundingTextPosition {
                input_context_path: path?,
                cursor,
                anchor,
            })
        }
        _ => None,
    }
}

// ====================================================================
// InputContextAllocator
// ====================================================================

#[derive(Debug, Default)]
pub struct InputContextAllocator {
    next_id: u64,
}

impl InputContextAllocator {
    pub const fn new() -> Self {
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

// ====================================================================
// build_reply
// ====================================================================

pub fn build_reply(
    request_bytes: &[u8],
    method: &Fcitx5MethodCall,
    ic_alloc: &mut InputContextAllocator,
) -> Option<Vec<u8>> {
    let request = msg_from_bytes(request_bytes)?;
    let header = request.header();

    let reply = match method {
        Fcitx5MethodCall::CreateInputContext { .. } => {
            let (path, uuid) = ic_alloc.allocate();
            let object_path = zvariant::ObjectPath::try_from(path.as_str()).ok()?;
            let uuid_vec = uuid.to_vec();
            Message::method_return(&header)
                .ok()?
                .build(&(object_path, uuid_vec))
                .ok()?
        }
        _ => Message::method_return(&header).ok()?.build(&()).ok()?,
    };

    Some(reply.data().to_vec())
}

// ====================================================================
// Tests
// ====================================================================

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    fn method_call<T>(iface: &str, member: &str, path: &str, body: &T) -> Vec<u8>
    where
        T: serde::Serialize + zvariant::DynamicType,
    {
        let msg = Message::method_call(path, member)
            .unwrap()
            .interface(iface)
            .unwrap()
            .destination("org.fcitx.Fcitx5")
            .unwrap()
            .build(body)
            .unwrap();
        msg.data().to_vec()
    }

    fn method_call_empty(iface: &str, member: &str, path: &str) -> Vec<u8> {
        let msg = Message::method_call(path, member)
            .unwrap()
            .interface(iface)
            .unwrap()
            .destination("org.fcitx.Fcitx5")
            .unwrap()
            .build(&())
            .unwrap();
        msg.data().to_vec()
    }

    fn ic_request(member: &str, path: &str, serial: u32) -> Vec<u8> {
        let msg = Message::method_call(path, member)
            .unwrap()
            .interface("org.fcitx.Fcitx.InputContext1")
            .unwrap()
            .destination("org.fcitx.Fcitx5")
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .serial(NonZeroU32::new(serial).unwrap())
            .build(&())
            .unwrap();
        msg.data().to_vec()
    }

    fn create_input_context_request(serial: u32) -> Vec<u8> {
        let hints: Vec<(String, String)> = Vec::new();
        let msg = Message::method_call("/org/freedesktop/portal/inputmethod", "CreateInputContext")
            .unwrap()
            .interface("org.fcitx.Fcitx.InputMethod1")
            .unwrap()
            .serial(NonZeroU32::new(serial).unwrap())
            .destination("org.fcitx.Fcitx5")
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .build(&hints)
            .unwrap();
        msg.data().to_vec()
    }

    // ----- Predicate tests -----

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

    // ----- Classify tests -----

    #[test]
    fn classifies_focus_in() {
        let bytes = method_call_empty(INPUT_CONTEXT_INTERFACE, "FocusIn", "/ic/7");
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::FocusIn {
                input_context_path: "/ic/7".into()
            })
        );
    }

    #[test]
    fn classifies_focus_out() {
        let bytes = method_call_empty(INPUT_CONTEXT_INTERFACE, "FocusOut", "/ic/1");
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::FocusOut {
                input_context_path: "/ic/1".into()
            })
        );
    }

    #[test]
    fn classifies_reset() {
        let bytes = method_call_empty(INPUT_CONTEXT_INTERFACE, "Reset", "/ic/1");
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::Reset {
                input_context_path: "/ic/1".into()
            })
        );
    }

    #[test]
    fn classifies_destroy_ic() {
        let bytes = method_call_empty(INPUT_CONTEXT_INTERFACE, "DestroyIC", "/ic/2");
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::DestroyIC {
                input_context_path: "/ic/2".into()
            })
        );
    }

    #[test]
    fn classifies_set_cursor_rect() {
        let bytes = method_call(
            INPUT_CONTEXT_INTERFACE,
            "SetCursorRect",
            "/ic/7",
            &(100i32, 200i32, 10i32, 20i32),
        );
        assert_eq!(
            classify(&bytes),
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
        let bytes = method_call(
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
        }) = classify(&bytes)
        else {
            panic!("not V2");
        };
        assert_eq!(input_context_path, "/ic/7");
        assert_eq!((x, y, w, h), (10, 20, 30, 40));
        assert_eq!(scale, 1.25);
    }

    #[test]
    fn classifies_fcitx4_set_cursor_location() {
        let bytes = method_call(
            INPUT_CONTEXT_INTERFACE_FCITX4,
            "SetCursorLocation",
            "/ic/7",
            &(50i32, 60i32),
        );
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::SetCursorLocation {
                input_context_path: "/ic/7".into(),
                x: 50,
                y: 60,
            })
        );
    }

    #[test]
    fn process_key_event_is_not_classified() {
        let msg = Message::method_call("/ic/7", "ProcessKeyEvent")
            .unwrap()
            .interface(INPUT_CONTEXT_INTERFACE)
            .unwrap()
            .destination("org.fcitx.Fcitx5")
            .unwrap()
            .build(&(0x61u32, 38u32, 0u32, false, 1234u32))
            .unwrap();
        let bytes = msg.data().to_vec();
        assert_eq!(classify(&bytes), None);
    }

    #[test]
    fn classifies_set_capability() {
        let bytes = method_call(
            INPUT_CONTEXT_INTERFACE,
            "SetCapability",
            "/ic/7",
            &0xDEADBEEFu64,
        );
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::SetCapability {
                input_context_path: "/ic/7".into(),
                capability: 0xDEADBEEF,
            })
        );
    }

    #[test]
    fn classifies_create_input_context_empty() {
        let hints: Vec<(String, String)> = Vec::new();
        let bytes = method_call(INPUT_METHOD_INTERFACE, "CreateInputContext", "/im", &hints);
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::CreateInputContext { hints: vec![] })
        );
    }

    #[test]
    fn classifies_create_input_context_with_one_hint() {
        let hints: Vec<(String, String)> = vec![("program".into(), "wechat".into())];
        let bytes = method_call(INPUT_METHOD_INTERFACE, "CreateInputContext", "/im", &hints);
        assert_eq!(
            classify(&bytes),
            Some(Fcitx5MethodCall::CreateInputContext {
                hints: vec![("program".into(), "wechat".into())],
            })
        );
    }

    #[test]
    fn unrelated_interface_is_not_classified() {
        let bytes = method_call_empty("org.freedesktop.DBus", "Hello", "/org/freedesktop/DBus");
        assert_eq!(classify(&bytes), None);
    }

    #[test]
    fn unknown_member_on_known_iface_is_not_classified() {
        let bytes = method_call(
            INPUT_CONTEXT_INTERFACE,
            "MysterySettings",
            "/ic/7",
            &(0i32, 0i32, 0i32),
        );
        assert_eq!(classify(&bytes), None);
    }

    // ----- Allocator tests -----

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

    // ----- Build reply tests -----

    #[test]
    fn create_input_context_returns_oay_with_swapped_endpoints() {
        let bytes = create_input_context_request(42);
        let mut alloc = InputContextAllocator::new();
        let reply_bytes = build_reply(
            &bytes,
            &Fcitx5MethodCall::CreateInputContext { hints: vec![] },
            &mut alloc,
        )
        .unwrap();
        let reply = msg_from_bytes(&reply_bytes).unwrap();
        assert_eq!(reply.message_type(), zbus::message::Type::MethodReturn);
        assert_eq!(reply.header().reply_serial().map(|s| s.get()), Some(42));
        assert_eq!(
            reply.header().destination().map(|d| d.as_str()),
            Some(":1.42")
        );
        assert_eq!(reply.header().signature().to_string_no_parens(), "oay");
    }

    #[test]
    fn create_input_context_allocates_fresh_path_each_call() {
        let mut alloc = InputContextAllocator::new();
        for serial_n in 1u32..=3 {
            let bytes = create_input_context_request(serial_n);
            build_reply(
                &bytes,
                &Fcitx5MethodCall::CreateInputContext { hints: vec![] },
                &mut alloc,
            );
        }
        let (path, _) = alloc.allocate();
        assert!(path.ends_with("/4"), "got {path}");
    }

    #[test]
    fn focus_in_returns_empty_method_return() {
        let bytes = ic_request("FocusIn", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply_bytes = build_reply(
            &bytes,
            &Fcitx5MethodCall::FocusIn {
                input_context_path: "/ic/7".into(),
            },
            &mut alloc,
        )
        .unwrap();
        let reply = msg_from_bytes(&reply_bytes).unwrap();
        assert_eq!(reply.message_type(), zbus::message::Type::MethodReturn);
        assert_eq!(reply.body().len(), 0);
        assert_eq!(reply.header().reply_serial().map(|s| s.get()), Some(1));
    }

    #[test]
    fn destroy_ic_returns_empty_method_return() {
        let bytes = ic_request("DestroyIC", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply_bytes = build_reply(
            &bytes,
            &Fcitx5MethodCall::DestroyIC {
                input_context_path: "/ic/7".into(),
            },
            &mut alloc,
        )
        .unwrap();
        let reply = msg_from_bytes(&reply_bytes).unwrap();
        assert_eq!(reply.message_type(), zbus::message::Type::MethodReturn);
        assert_eq!(reply.body().len(), 0);
    }

    #[test]
    fn set_cursor_rect_v2_returns_empty_method_return() {
        let bytes = ic_request("SetCursorRectV2", "/ic/7", 1);
        let mut alloc = InputContextAllocator::new();
        let reply_bytes = build_reply(
            &bytes,
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
        let reply = msg_from_bytes(&reply_bytes).unwrap();
        assert_eq!(reply.body().len(), 0);
    }
}
