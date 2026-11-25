//! Attache `WKScriptMessageHandler` `streamcraftMedia` + user script all-frames.
//! Port exact de `apps/desktop/src-tauri/src/macos_media_hook.rs` (Tauri) /
//! Swift `NetworkObserver.attachScriptMessaging` (D008).
//!
//! Callers: `browser/mod.rs` `EmbeddedBrowser::create` (après `build_as_child`).
//! Pas de fichier équivalent dans egui avant — d’où Detected vide sur rizmov.
//! User: "tu peux pas relire ce qui etait fait dans la version swift et dans la version Tauri???"
#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use objc2::{DeclaredClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime,
};
use streamcraft_core::agent::popup_guard::PopupGuard;

#[derive(Debug, Clone)]
pub struct ScriptMediaPayload {
    pub url: String,
    pub mime: Option<String>,
    pub source: Option<String>,
    pub page: Option<String>,
}

pub type PendingMedia = Arc<Mutex<Vec<ScriptMediaPayload>>>;

pub struct StreamCraftMediaHandlerIvars {
    pub pending: PendingMedia,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = StreamCraftMediaHandlerIvars]
    pub struct StreamCraftMediaHandler;

    unsafe impl NSObjectProtocol for StreamCraftMediaHandler {}

    unsafe impl WKScriptMessageHandler for StreamCraftMediaHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(
            this: &StreamCraftMediaHandler,
            _controller: &WKUserContentController,
            msg: &WKScriptMessage,
        ) {
            let body = unsafe { msg.body() };
            let Some(dict) = body.downcast_ref::<NSDictionary>() else {
                return;
            };
            let url = dict_string(dict, "url");
            let Some(url) = url else { return };
            let mime = dict_string(dict, "mime");
            let source = dict_string(dict, "source");
            let page = dict_string(dict, "page");
            let payload = ScriptMediaPayload {
                url,
                mime,
                source,
                page,
            };
            tracing::debug!(url = %payload.url, "streamcraftMedia WK message");
            if let Ok(mut g) = this.ivars().pending.lock() {
                g.push(payload);
            }
        }
    }
);

fn dict_string(dict: &NSDictionary, key: &str) -> Option<String> {
    let k = NSString::from_str(key);
    let v = dict.objectForKey(&*k)?;
    let s = v.downcast_ref::<NSString>()?;
    Some(s.to_string())
}

/// Register handler + inject NetworkObserver JS for **all frames** (Swift D008 / Tauri).
/// wry: `WebViewExtMacOS::manager()` == Tauri `WKWebView.configuration.userContentController`.
pub fn attach(
    controller: &WKUserContentController,
    pending: PendingMedia,
) -> Option<Retained<StreamCraftMediaHandler>> {
    unsafe {
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        let handler = mtm
            .alloc::<StreamCraftMediaHandler>()
            .set_ivars(StreamCraftMediaHandlerIvars { pending });
        let handler: Retained<StreamCraftMediaHandler> = msg_send![super(handler), init];

        let name = NSString::from_str("streamcraftMedia");
        controller.removeScriptMessageHandlerForName(&name);
        let proto = objc2::runtime::ProtocolObject::from_ref(&*handler);
        controller.addScriptMessageHandler_name(proto, &name);

        // Exact Swift: atDocumentStart, forMainFrameOnly: false
        let src = NSString::from_str(PopupGuard::MEDIA_HOOKS_JS);
        let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            mtm.alloc::<WKUserScript>(),
            &src,
            WKUserScriptInjectionTime::AtDocumentStart,
            false,
        );
        controller.addUserScript(&script);

        std::mem::forget(handler.clone());
        Some(handler)
    }
}
