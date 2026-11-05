//! Activation of the bundled CoreMediaIO Camera Extension through
//! `OSSystemExtensionManager`. Only works when running from the signed
//! `CameraCenter.app` bundle in `/Applications` (see `platform/macos`).

use std::sync::{Arc, Mutex, OnceLock};

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};
use objc2_system_extensions::{
    OSSystemExtensionManager, OSSystemExtensionProperties, OSSystemExtensionReplacementAction,
    OSSystemExtensionRequest, OSSystemExtensionRequestDelegate, OSSystemExtensionRequestResult,
};
use tracing::{info, warn};

/// Bundle identifier of the camera extension (see platform/macos/project.yml).
pub const EXTENSION_BUNDLE_ID: &str = "com.cameracenter.app.camera-extension";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationState {
    Idle,
    Submitted,
    /// The user must allow the extension in System Settings → General →
    /// Login Items & Extensions → Camera Extensions.
    NeedsApproval,
    Activated,
    /// Activated but takes effect after a reboot.
    RebootRequired,
    Failed(String),
}

impl ActivationState {
    pub fn describe(&self) -> String {
        match self {
            Self::Idle => "Not requested".into(),
            Self::Submitted => "Installing…".into(),
            Self::NeedsApproval => {
                "Approve the extension in System Settings → General → Login Items & Extensions"
                    .into()
            }
            Self::Activated => "Extension installed".into(),
            Self::RebootRequired => "Installed, restart required".into(),
            Self::Failed(e) => format!("Install failed: {e}"),
        }
    }
}

fn state() -> &'static Arc<Mutex<ActivationState>> {
    static STATE: OnceLock<Arc<Mutex<ActivationState>>> = OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(ActivationState::Idle)))
}

fn set_state(s: ActivationState) {
    info!(state = ?s, "camera extension activation");
    *state().lock().unwrap_or_else(|e| e.into_inner()) = s;
}

pub fn activation_state() -> ActivationState {
    state().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub struct Ivars;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and we don't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CCSystemExtensionDelegate"]
    #[ivars = Ivars]
    pub struct ExtensionDelegate;

    unsafe impl NSObjectProtocol for ExtensionDelegate {}

    unsafe impl OSSystemExtensionRequestDelegate for ExtensionDelegate {
        #[unsafe(method(request:actionForReplacingExtension:withExtension:))]
        fn action_for_replacing(
            &self,
            _request: &OSSystemExtensionRequest,
            _existing: &OSSystemExtensionProperties,
            _replacement: &OSSystemExtensionProperties,
        ) -> OSSystemExtensionReplacementAction {
            OSSystemExtensionReplacementAction::Replace
        }

        #[unsafe(method(requestNeedsUserApproval:))]
        fn needs_user_approval(&self, _request: &OSSystemExtensionRequest) {
            set_state(ActivationState::NeedsApproval);
        }

        #[unsafe(method(request:didFinishWithResult:))]
        fn did_finish(
            &self,
            _request: &OSSystemExtensionRequest,
            result: OSSystemExtensionRequestResult,
        ) {
            if result == OSSystemExtensionRequestResult::WillCompleteAfterReboot {
                set_state(ActivationState::RebootRequired);
            } else {
                set_state(ActivationState::Activated);
            }
        }

        #[unsafe(method(request:didFailWithError:))]
        fn did_fail(&self, _request: &OSSystemExtensionRequest, error: &NSError) {
            let msg = error.localizedDescription().to_string();
            warn!(code = error.code(), %msg, "camera extension activation failed");
            set_state(ActivationState::Failed(explain(&msg)));
        }
    }
);

impl ExtensionDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars);
        // SAFETY: NSObject's init is always safe to call on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

/// Command that produces an installable, signed build.
pub const INSTALL_HINT: &str = "build and install the signed app with `TEAM_ID=<your team> just install-osx`, then open /Applications/CameraCenter.app";

/// macOS only installs system extensions requested by an app bundle located
/// in /Applications. Returns why the current process can't, if it can't.
pub fn install_blocker() -> Option<String> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Some(format!("cannot locate executable: {e}")),
    };
    let path = exe.to_string_lossy();
    if !path.contains(".app/Contents/MacOS/") {
        return Some(format!("Running outside the app bundle; {INSTALL_HINT}"));
    }
    if !path.starts_with("/Applications/") {
        return Some(format!(
            "The app must run from /Applications; {INSTALL_HINT}"
        ));
    }
    None
}

/// Rewrites common activation errors into something actionable.
fn explain(error: &str) -> String {
    if error.contains("entitlement") {
        format!("this build isn't signed with the system extension entitlement; {INSTALL_HINT}")
    } else {
        error.to_string()
    }
}

/// Requests activation (install or update) of the camera extension.
/// Progress is reported through [`activation_state`].
pub fn activate() {
    if let Some(reason) = install_blocker() {
        warn!("camera extension cannot be installed: {reason}");
        set_state(ActivationState::Failed(reason));
        return;
    }
    // The request only holds a weak reference to its delegate.
    static DELEGATE: OnceLock<usize> = OnceLock::new();
    let delegate_ptr =
        *DELEGATE.get_or_init(|| Retained::into_raw(ExtensionDelegate::new()) as usize);
    // SAFETY: the pointer was produced by Retained::into_raw above and is never freed.
    let delegate: &ExtensionDelegate = unsafe { &*(delegate_ptr as *const ExtensionDelegate) };

    set_state(ActivationState::Submitted);
    let identifier = NSString::from_str(EXTENSION_BUNDLE_ID);
    let queue = DispatchQueue::main();
    // SAFETY: plain Objective-C calls with valid arguments; callbacks arrive
    // on the main queue, which the UI event loop drains.
    unsafe {
        let request =
            OSSystemExtensionRequest::activationRequestForExtension_queue(&identifier, queue);
        request.setDelegate(Some(ProtocolObject::from_ref(delegate)));
        OSSystemExtensionManager::sharedManager().submitRequest(&request);
    }
}
