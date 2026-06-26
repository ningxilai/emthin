pub mod fcitx;
pub mod proxy;
pub mod router;

pub use fcitx::{
    build_preedit_chunks, build_reply, classify, method_call_to_event, Fcitx5MethodCall,
    FcitxEvent, InputContextAllocator, INPUT_CONTEXT_INTERFACE, INPUT_CONTEXT_INTERFACE_FCITX4,
    INPUT_CONTEXT_PATH_PREFIX, INPUT_METHOD_INTERFACE,
};
pub use router::{RouteRule, RouterNotification, RouterRequest, RoutingTable};
