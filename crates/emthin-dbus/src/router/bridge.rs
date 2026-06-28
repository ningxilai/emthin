use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use gio::prelude::*;
use gio::{
    DBusConnection, DBusMessage, DBusMessageType, DBusSendMessageFlags, DBusServer, DBusServerFlags,
};

use crate::fcitx::{self, Fcitx5MethodCall, FcitxEvent, InputContextAllocator};
use crate::router::rule::{RouteRule, RoutingTable};

#[derive(Debug)]
pub enum BridgeCommand {
    ImeCommit {
        ic_path: String,
        text: String,
    },
    ImePreedit {
        ic_path: String,
        text: String,
        cursor_begin: i32,
        cursor_end: i32,
    },
    AddRule(RouteRule),
    RemoveRule(String),
    ListRules,
    Shutdown,
}

#[derive(Debug)]
pub enum BridgeNotification {
    FcitxEvent(FcitxEvent),
    RuleAdded { id: String, rule: RouteRule },
    RuleRemoved { id: String },
    RuleList { rules: Vec<RouteRule> },
}

type IcRegistry = Arc<Mutex<HashMap<String, DBusConnection>>>;
type Routing = Arc<Mutex<RoutingTable>>;

type CmdSender = mpsc::Sender<BridgeCommand>;
#[allow(dead_code)]
type CmdReceiver = mpsc::Receiver<BridgeCommand>;
type NotifySender = mpsc::Sender<BridgeNotification>;
type NotifyReceiver = mpsc::Receiver<BridgeNotification>;

pub fn spawn(listen_path: PathBuf, upstream_path: PathBuf) -> (CmdSender, NotifyReceiver) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<BridgeCommand>();
    let (notify_tx, notify_rx) = mpsc::channel::<BridgeNotification>();

    let listen_addr = format!("unix:path={}", listen_path.display());
    std::thread::spawn(move || {
        let ctx = glib::MainContext::new();
        let _ = ctx.with_thread_default(|| {
            let guid = "e1e5a7b3c4d8f9a0123456789abcdef0";
            let server = match DBusServer::new_sync(
                &listen_addr,
                DBusServerFlags::AUTHENTICATION_ALLOW_ANONYMOUS,
                guid,
                None::<&gio::DBusAuthObserver>,
                None::<&gio::Cancellable>,
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, ?listen_path, "failed to create DBusServer");
                    return;
                }
            };

            let ic_fds: IcRegistry = Arc::new(Mutex::new(HashMap::new()));
            let routing_table: Routing = Arc::new(Mutex::new(RoutingTable::default()));

            let srv_ic = ic_fds.clone();
            let srv_notify = notify_tx.clone();
            let srv_routing = routing_table.clone();
            let srv_upstream = upstream_path.clone();

            server.connect_new_connection(move |_srv, conn| {
                let upstream_addr = format!("unix:path={}", srv_upstream.to_string_lossy());
                let upstream = match DBusConnection::for_address_sync(
                    &upstream_addr,
                    gio::DBusConnectionFlags::AUTHENTICATION_CLIENT,
                    None::<&gio::DBusAuthObserver>,
                    None::<&gio::Cancellable>,
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "upstream connect failed");
                        return false;
                    }
                };

                let cl_ic = srv_ic.clone();
                let cl_notify = srv_notify.clone();
                let cl_routing = srv_routing.clone();
                let cl_upstream = upstream.clone();

                let _filter_id = conn.add_filter(move |conn, msg, incoming| {
                    handle_client_message(
                        conn,
                        msg,
                        incoming,
                        &cl_upstream,
                        &cl_ic,
                        &cl_routing,
                        &cl_notify,
                    )
                });

                let cl_conn = conn.clone();
                upstream.add_filter(move |_up, msg, incoming| {
                    if incoming && msg.message_type() == DBusMessageType::Signal {
                        if let Ok(copy) = msg.copy() {
                            let _ = cl_conn.send_message(&copy, DBusSendMessageFlags::NONE);
                        }
                    }
                    Some(msg.clone())
                });

                upstream.start_message_processing();
                true
            });

            server.start();
            tracing::info!(?listen_path, ?upstream_path, "dbus bridge started");

            let loop_ = glib::MainLoop::new(Some(&ctx), false);
            let l = loop_.clone();

            let cmd_ic = ic_fds.clone();
            let cmd_routing = routing_table.clone();
            let cmd_notify = notify_tx.clone();
            std::thread::spawn(move || loop {
                match cmd_rx.recv() {
                    Ok(BridgeCommand::Shutdown) => {
                        l.quit();
                        break;
                    }
                    Ok(cmd) => handle_bridge_command(cmd, &cmd_ic, &cmd_routing, &cmd_notify),
                    Err(_) => break,
                }
            });

            loop_.run();
        });
    });

    (cmd_tx, notify_rx)
}

fn handle_client_message(
    _conn: &DBusConnection,
    msg: &DBusMessage,
    incoming: bool,
    upstream: &DBusConnection,
    ic_fds: &IcRegistry,
    routing_table: &Routing,
    notify: &NotifySender,
) -> Option<DBusMessage> {
    if !incoming {
        return Some(msg.clone());
    }

    if msg.message_type() != DBusMessageType::MethodCall {
        return Some(msg.clone());
    }

    let iface = msg.interface().map(|i| i.to_string());

    let routing_guard = routing_table.lock().unwrap();
    let route = routing_guard.route_str(
        msg.destination().as_deref(),
        iface.as_deref(),
        msg.member().as_deref(),
    );

    if let Some("deny") = route {
        let err = msg.new_method_error_literal(
            "org.freedesktop.DBus.Error.AccessDenied",
            "blocked by routing rule",
        );
        let _ = _conn.send_message(&err, DBusSendMessageFlags::NONE);
        return None;
    }

    if let Some(ref iface) = iface {
        if fcitx::is_fcitx_interface(iface) {
            if let Some(method) = fcitx::classify(msg) {
                let mut alloc = InputContextAllocator::new();
                if let Some(reply) = fcitx::build_reply(msg, &method, &mut alloc) {
                    if let Fcitx5MethodCall::CreateInputContext { .. } = &method {
                        let path = reply
                            .body()
                            .map(|b| b.child_value(0))
                            .and_then(|v: glib::Variant| v.get::<String>());
                        if let Some(p) = path {
                            ic_fds.lock().unwrap().insert(p, upstream.clone());
                        }
                    }
                    let _ = _conn.send_message(&reply, DBusSendMessageFlags::NONE);
                }
                if let Some(event) = fcitx::method_call_to_event(&method) {
                    let _ = notify.send(BridgeNotification::FcitxEvent(event));
                }
                return None;
            }
        }

        if iface == "org.freedesktop.DBus" && route.is_none() {
            return Some(msg.clone());
        }
    }

    let unlocked = match msg.copy() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to copy message for upstream");
            return None;
        }
    };
    match upstream.send_message_with_reply_sync(
        &unlocked,
        DBusSendMessageFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
    ) {
        Ok((reply, _serial)) => {
            let _ = _conn.send_message(&reply, DBusSendMessageFlags::NONE);
        }
        Err(e) => {
            tracing::warn!(error = %e, "upstream call failed");
            let err = msg.new_method_error_literal(
                "org.freedesktop.DBus.Error.Failed",
                &format!("upstream call failed: {e}"),
            );
            let _ = _conn.send_message(&err, DBusSendMessageFlags::NONE);
        }
    }
    None
}

fn handle_bridge_command(
    cmd: BridgeCommand,
    ic_fds: &IcRegistry,
    routing_table: &Routing,
    notify: &NotifySender,
) {
    match cmd {
        BridgeCommand::AddRule(rule) => {
            let id = rule.id.clone();
            routing_table.lock().unwrap().add(rule.clone());
            let _ = notify.send(BridgeNotification::RuleAdded { id, rule });
        }
        BridgeCommand::RemoveRule(id) => {
            routing_table.lock().unwrap().remove(&id);
            let _ = notify.send(BridgeNotification::RuleRemoved { id });
        }
        BridgeCommand::ListRules => {
            let rules = routing_table.lock().unwrap().rules().to_vec();
            let _ = notify.send(BridgeNotification::RuleList { rules });
        }
        BridgeCommand::ImeCommit { ic_path, text } => {
            let fds = ic_fds.lock().unwrap();
            if let Some(conn) = fds.get(&ic_path) {
                let msg = DBusMessage::new_signal(
                    &ic_path,
                    fcitx::INPUT_CONTEXT_INTERFACE,
                    "CommitString",
                );
                let v = text.to_variant();
                msg.set_body(&v);
                let _ = conn.send_message(&msg, DBusSendMessageFlags::NONE);
            }
        }
        BridgeCommand::ImePreedit {
            ic_path,
            text,
            cursor_begin,
            cursor_end,
        } => {
            let fds = ic_fds.lock().unwrap();
            if let Some(conn) = fds.get(&ic_path) {
                let cursor = if cursor_begin != cursor_end || cursor_begin >= 0 {
                    Some((cursor_begin, cursor_end))
                } else {
                    None
                };
                let underline = 1 << 3;
                let highlight = 1 << 4;
                let chunks = fcitx::build_preedit_chunks(&text, cursor, underline, highlight);
                let cursor_offset = cursor.map(|(_, e)| e).unwrap_or(-1);
                let msg = DBusMessage::new_signal(
                    &ic_path,
                    fcitx::INPUT_CONTEXT_INTERFACE,
                    "UpdateFormattedPreedit",
                );
                let body = (chunks, cursor_offset).to_variant();
                msg.set_body(&body);
                let _ = conn.send_message(&msg, DBusSendMessageFlags::NONE);
            }
        }
        BridgeCommand::Shutdown => unreachable!(),
    }
}
