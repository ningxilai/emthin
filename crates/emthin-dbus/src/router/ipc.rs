//! JSON-RPC IPC messages between the emthin main process and the
//! emthin-dbus-router subprocess.

use serde::{Deserialize, Serialize};

use crate::fcitx::FcitxEvent;

/// A single routing rule. Serialised as JSON over the IPC channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub id: String,
    pub priority: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub target: String, // "host" | "isolated" | "deny"
}

/// Requests sent from emthin main → router.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum RouterRequest {
    AddRule { rule: RouteRule },
    RemoveRule { id: String },
    ListRules,
    #[serde(rename = "ime_commit")]
    ImeCommit { ic_path: String, text: String },
    #[serde(rename = "ime_preedit")]
    ImePreedit {
        ic_path: String,
        text: String,
        cursor_begin: i32,
        cursor_end: i32,
    },
}

/// Notifications sent from router → emthin main.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum RouterNotification {
    #[serde(rename = "fcitx_event")]
    FcitxEvent(FcitxEvent),
    #[serde(rename = "rule_added")]
    RuleAdded {
        id: String,
        rule: RouteRule,
    },
    #[serde(rename = "rule_removed")]
    RuleRemoved { id: String },
}
