/// Geometry rectangle from Emacs IPC (logical pixels, Emacs-relative).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpcRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Emacs → emthin
#[derive(Debug)]
pub enum IncomingMessage {
    SetGeometry {
        window_id: u64,
        rect: IpcRect,
    },
    Close {
        window_id: u64,
    },
    SetVisibility {
        window_id: u64,
        visible: bool,
    },
    /// Emacs finished processing a prefix key sequence in an embedded
    /// app buffer; clear `prefix_active` AND restore the saved app focus.
    PrefixDone,
    /// Emacs finished any command (global hook); only clear
    /// `prefix_active` so host IME can resume — focus is left
    /// wherever Emacs's prefix command put it.
    PrefixClear,
    AddMirror {
        window_id: u64,
        view_id: u64,
        rect: IpcRect,
    },
    UpdateMirrorGeometry {
        window_id: u64,
        view_id: u64,
        rect: IpcRect,
    },
    RemoveMirror {
        window_id: u64,
        view_id: u64,
    },
    /// Source was deleted; promote this mirror to become the new source.
    PromoteMirror {
        window_id: u64,
        view_id: u64,
    },
    /// Tell the compositor which surface should have keyboard focus.
    /// `window_id: None` means focus Emacs; `Some(id)` means focus that app.
    SetFocus {
        window_id: Option<u64>,
    },
    /// Request the compositor to switch to the given workspace.
    SwitchWorkspace {
        workspace_id: u64,
    },
    /// Add a DBus routing rule.
    DbusRouterAddRule {
        rule: emthin_dbus::router::RouteRule,
    },
    /// Remove a DBus routing rule by id.
    DbusRouterRemoveRule {
        id: String,
    },
    /// List all current DBus routing rules.
    DbusRouterListRules,
}

/// emthin → Emacs
#[derive(Debug, Clone)]
pub enum OutgoingMessage {
    Connected {
        version: &'static str,
    },
    WindowCreated {
        window_id: u64,
        title: String,
    },
    WindowDestroyed {
        window_id: u64,
    },
    TitleChanged {
        window_id: u64,
        title: String,
    },
    /// Emacs surface logical size (so Emacs can compute header offset).
    SurfaceSize {
        width: i32,
        height: i32,
    },
    /// User clicked on an embedded app — Emacs should select the corresponding window.
    /// view_id=0 means the source window; otherwise it's a mirror view_id.
    FocusView {
        window_id: u64,
        view_id: u64,
    },
    /// XWayland is ready — Emacs can set DISPLAY=:<display> for X11 apps.
    XWaylandReady {
        display: u32,
    },
    /// A new workspace was created (new Emacs frame detected).
    WorkspaceCreated {
        workspace_id: u64,
    },
    /// The active workspace changed.
    WorkspaceSwitched {
        workspace_id: u64,
    },
    /// A workspace was destroyed (Emacs frame closed).
    WorkspaceDestroyed {
        workspace_id: u64,
    },
    /// Current DBus routing rules (response to ListRules).
    DbusRouterRules {
        rules: Vec<emthin_dbus::router::RouteRule>,
    },
    /// A rule was added.
    DbusRouterRuleAdded {
        id: String,
        rule: emthin_dbus::router::RouteRule,
    },
    /// A rule was removed.
    DbusRouterRuleRemoved {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Manual JSON-RPC conversion (no serde derive)
// ---------------------------------------------------------------------------

impl IncomingMessage {
    pub fn from_jsonrpc(method: &str, params: &serde_json::Value) -> Result<Self, String> {
        Ok(match method {
            "set_geometry" => Self::SetGeometry {
                window_id: params_get_u64(params, "window_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "close" => Self::Close {
                window_id: params_get_u64(params, "window_id")?,
            },
            "set_visibility" => Self::SetVisibility {
                window_id: params_get_u64(params, "window_id")?,
                visible: params_get_bool(params, "visible")?,
            },
            "prefix_done" => Self::PrefixDone,
            "prefix_clear" => Self::PrefixClear,
            "add_mirror" => Self::AddMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "update_mirror_geometry" => Self::UpdateMirrorGeometry {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "remove_mirror" => Self::RemoveMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
            },
            "promote_mirror" => Self::PromoteMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
            },
            "set_focus" => Self::SetFocus {
                window_id: params.get("window_id").and_then(|v| v.as_u64()),
            },
            "switch_workspace" => Self::SwitchWorkspace {
                workspace_id: params_get_u64(params, "workspace_id")?,
            },
            "dbus_router_add_rule" => {
                let rule: emthin_dbus::router::RouteRule =
                    serde_json::from_value(params["rule"].clone())
                        .map_err(|e| format!("invalid rule: {e}"))?;
                Self::DbusRouterAddRule { rule }
            }
            "dbus_router_remove_rule" => Self::DbusRouterRemoveRule {
                id: params_get_string(params, "id")?,
            },
            "dbus_router_list_rules" => Self::DbusRouterListRules,
            other => return Err(format!("unknown IPC method: {other}")),
        })
    }
}

fn params_get_u64(params: &serde_json::Value, key: &str) -> Result<u64, String> {
    params[key]
        .as_u64()
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
}

fn params_get_i32(params: &serde_json::Value, key: &str) -> Result<i32, String> {
    params[key]
        .as_i64()
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
        .map(|v| v as i32)
}

fn params_get_bool(params: &serde_json::Value, key: &str) -> Result<bool, String> {
    params[key]
        .as_bool()
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
}

fn params_get_string(params: &serde_json::Value, key: &str) -> Result<String, String> {
    params[key]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
}

impl OutgoingMessage {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::WindowCreated { .. } => "window_created",
            Self::WindowDestroyed { .. } => "window_destroyed",
            Self::TitleChanged { .. } => "title_changed",
            Self::SurfaceSize { .. } => "surface_size",
            Self::FocusView { .. } => "focus_view",
            Self::XWaylandReady { .. } => "x_wayland_ready",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceSwitched { .. } => "workspace_switched",
            Self::WorkspaceDestroyed { .. } => "workspace_destroyed",
            Self::DbusRouterRules { .. } => "dbus_router_rules",
            Self::DbusRouterRuleAdded { .. } => "dbus_router_rule_added",
            Self::DbusRouterRuleRemoved { .. } => "dbus_router_rule_removed",
        }
    }

    pub fn into_params_value(self) -> serde_json::Value {
        match self {
            Self::Connected { version } => serde_json::json!({"version": version}),
            Self::WindowCreated { window_id, title } => {
                serde_json::json!({"window_id": window_id, "title": title})
            }
            Self::WindowDestroyed { window_id } => {
                serde_json::json!({"window_id": window_id})
            }
            Self::TitleChanged { window_id, title } => {
                serde_json::json!({"window_id": window_id, "title": title})
            }
            Self::SurfaceSize { width, height } => {
                serde_json::json!({"width": width, "height": height})
            }
            Self::FocusView { window_id, view_id } => {
                serde_json::json!({"window_id": window_id, "view_id": view_id})
            }
            Self::XWaylandReady { display } => serde_json::json!({"display": display}),
            Self::WorkspaceCreated { workspace_id } => {
                serde_json::json!({"workspace_id": workspace_id})
            }
            Self::WorkspaceSwitched { workspace_id } => {
                serde_json::json!({"workspace_id": workspace_id})
            }
            Self::WorkspaceDestroyed { workspace_id } => {
                serde_json::json!({"workspace_id": workspace_id})
            }
            Self::DbusRouterRules { rules } => {
                serde_json::json!({"rules": serde_json::to_value(rules).unwrap_or_default()})
            }
            Self::DbusRouterRuleAdded { id, rule } => serde_json::json!({
                "id": id,
                "rule": serde_json::to_value(rule).unwrap_or_default(),
            }),
            Self::DbusRouterRuleRemoved { id } => {
                serde_json::json!({"id": id})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_geometry() {
        let params = serde_json::json!({"window_id":42,"x":10,"y":20,"w":800,"h":600});
        let msg = IncomingMessage::from_jsonrpc("set_geometry", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetGeometry {
                window_id: 42,
                rect: IpcRect {
                    x: 10,
                    y: 20,
                    w: 800,
                    h: 600
                }
            }
        ));
    }

    #[test]
    fn parses_close() {
        let params = serde_json::json!({"window_id":7});
        let msg = IncomingMessage::from_jsonrpc("close", &params).unwrap();
        assert!(matches!(msg, IncomingMessage::Close { window_id: 7 }));
    }

    #[test]
    fn parses_set_visibility() {
        let params = serde_json::json!({"window_id":3,"visible":false});
        let msg = IncomingMessage::from_jsonrpc("set_visibility", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetVisibility {
                window_id: 3,
                visible: false
            }
        ));
    }

    #[test]
    fn parses_prefix_done() {
        let msg = IncomingMessage::from_jsonrpc("prefix_done", &serde_json::Value::Null).unwrap();
        assert!(matches!(msg, IncomingMessage::PrefixDone));
    }

    #[test]
    fn parses_add_mirror() {
        let params = serde_json::json!({"window_id":1,"view_id":2,"x":0,"y":0,"w":400,"h":300});
        let msg = IncomingMessage::from_jsonrpc("add_mirror", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::AddMirror {
                window_id: 1,
                view_id: 2,
                ..
            }
        ));
    }

    #[test]
    fn parses_set_focus_with_window_id() {
        let params = serde_json::json!({"window_id":9});
        let msg = IncomingMessage::from_jsonrpc("set_focus", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetFocus { window_id: Some(9) }
        ));
    }

    #[test]
    fn parses_set_focus_without_window_id() {
        let params = serde_json::json!({});
        let msg = IncomingMessage::from_jsonrpc("set_focus", &params).unwrap();
        assert!(matches!(msg, IncomingMessage::SetFocus { window_id: None }));
    }

    #[test]
    fn parses_switch_workspace() {
        let params = serde_json::json!({"workspace_id":5});
        let msg = IncomingMessage::from_jsonrpc("switch_workspace", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SwitchWorkspace { workspace_id: 5 }
        ));
    }

    #[test]
    fn rejects_unknown_method() {
        let result = IncomingMessage::from_jsonrpc("unknown_command", &serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_missing_required_fields() {
        let params = serde_json::json!({"window_id":1});
        let result = IncomingMessage::from_jsonrpc("set_geometry", &params);
        assert!(result.is_err());
    }

    #[test]
    fn outgoing_method_name() {
        assert_eq!(
            OutgoingMessage::Connected { version: "0.1" }.method_name(),
            "connected"
        );
        assert_eq!(
            OutgoingMessage::WindowCreated {
                window_id: 1,
                title: "t".into()
            }
            .method_name(),
            "window_created"
        );
        assert_eq!(
            OutgoingMessage::XWaylandReady { display: 42 }.method_name(),
            "x_wayland_ready"
        );
        assert_eq!(
            OutgoingMessage::SurfaceSize {
                width: 1920,
                height: 1080
            }
            .method_name(),
            "surface_size"
        );
    }

    #[test]
    fn outgoing_into_params_value() {
        let v = OutgoingMessage::Connected { version: "0.1" }.into_params_value();
        assert_eq!(v["version"], "0.1");
        let v = OutgoingMessage::WindowCreated {
            window_id: 42,
            title: "test".into(),
        }
        .into_params_value();
        assert_eq!(v["window_id"], 42);
        assert_eq!(v["title"], "test");
        let v = OutgoingMessage::XWaylandReady { display: 99 }.into_params_value();
        assert_eq!(v["display"], 99);
    }
}
