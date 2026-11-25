//! Attache `WKScriptMessageHandler` `streamcraftMedia` + user script all-frames
//! — clone exact Swift `NetworkObserver.attachScriptMessaging`.
#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use objc2::{DeclaredClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView,
};

use crate::ScriptMediaPayload;
use streamcraft_core::agent::popup_guard::PopupGuard;

pub type PendingMedia = Arc<Mutex<Vec<ScriptMediaPayload>>>;

pub struct OpenStreamMediaHandlerIvars {
    pub pending: PendingMedia,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = OpenStreamMediaHandlerIvars]
    pub struct OpenStreamMediaHandler;

    unsafe impl NSObjectProtocol for OpenStreamMediaHandler {}

    unsafe impl WKScriptMessageHandler for OpenStreamMediaHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(
            this: &OpenStreamMediaHandler,
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
            eprintln!("[streamcraftMedia] {}", payload.url);
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

/// Register handler + inject NetworkObserver JS for **all frames** (Swift D008).
pub fn attach(webview_ptr: *mut std::ffi::c_void, pending: PendingMedia) -> Option<Retained<OpenStreamMediaHandler>> {
    if webview_ptr.is_null() {
        return None;
    }
    unsafe {
        let view: &WKWebView = &*webview_ptr.cast::<WKWebView>();
        let config = view.configuration();
        let controller = config.userContentController();

        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        let handler = mtm
            .alloc::<OpenStreamMediaHandler>()
            .set_ivars(OpenStreamMediaHandlerIvars { pending });
        let handler: Retained<OpenStreamMediaHandler> = msg_send![super(handler), init];

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
