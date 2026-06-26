use crate::ipc::{IncomingMessage, OutgoingMessage};

/// Parse a JSON-RPC 2.0 notification payload into an `IncomingMessage`.
pub fn parse_incoming(payload: &[u8]) -> Result<IncomingMessage, String> {
    let v: serde_json::Value =
        serde_json::from_slice(payload).map_err(|e| format!("JSON parse error: {e}"))?;
    let jsonrpc = v.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        return Err(format!("invalid jsonrpc version: {jsonrpc:?}"));
    }
    let method = v["method"]
        .as_str()
        .ok_or_else(|| "missing 'method' field".to_string())?;
    let params = v.get("params").unwrap_or(&serde_json::Value::Null);
    IncomingMessage::from_jsonrpc(method, params)
}

/// Serialize an `OutgoingMessage` as a JSON-RPC 2.0 notification.
pub fn serialize_outgoing(msg: OutgoingMessage) -> Result<Vec<u8>, serde_json::Error> {
    let method = msg.method_name();
    let params = msg.into_params_value();
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    serde_json::to_vec(&envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcRect;

    #[test]
    fn roundtrip_connected() {
        let msg = OutgoingMessage::Connected { version: "0.1" };
        let wire = serialize_outgoing(msg).unwrap();
        let wire_str = String::from_utf8_lossy(&wire);
        assert!(wire_str.contains(r#""jsonrpc":"2.0""#));
        assert!(wire_str.contains(r#""method":"connected""#));
        assert!(wire_str.contains(r#""version":"0.1""#));
    }

    #[test]
    fn parse_set_geometry() {
        let wire = br#"{"jsonrpc":"2.0","method":"set_geometry","params":{"window_id":42,"x":0.5,"y":0.3,"w":0.4,"h":0.6}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetGeometry {
                window_id: 42,
                rect: IpcRect {
                    x: 0.5,
                    y: 0.3,
                    w: 0.4,
                    h: 0.6
                }
            }
        ));
    }

    #[test]
    fn parse_close() {
        let wire = br#"{"jsonrpc":"2.0","method":"close","params":{"window_id":7}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::Close { window_id: 7 }));
    }

    #[test]
    fn parse_prefix_done() {
        let wire = br#"{"jsonrpc":"2.0","method":"prefix_done","params":null}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::PrefixDone));
    }

    #[test]
    fn parse_set_focus_no_window_id() {
        let wire = br#"{"jsonrpc":"2.0","method":"set_focus","params":{}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::SetFocus { window_id: None }));
    }

    #[test]
    fn rejects_missing_jsonrpc_field() {
        let wire = br#"{"method":"close","params":{"window_id":1}}"#;
        let result = parse_incoming(wire);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_method() {
        let wire = br#"{"jsonrpc":"2.0","method":"bogus","params":{}}"#;
        let result = parse_incoming(wire);
        assert!(result.is_err());
    }
}
