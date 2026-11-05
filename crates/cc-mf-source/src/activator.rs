use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFActivate_Impl, IMFAttributes, IMFMediaSource, MF_ATTRIBUTE_TYPE,
    MF_ATTRIBUTES_MATCH_TYPE, MFCreateAttributes,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{BOOL, GUID, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, implement};

use crate::attributes::forward_attributes;
use crate::source::MediaSource;

/// Object returned by the class factory; Frame Server activates the media
/// source through it.
#[implement(IMFActivate)]
pub struct Activator {
    attributes: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

impl Activator {
    pub fn new() -> Result<Self> {
        let mut attributes = None;
        // SAFETY: out pointer is valid.
        unsafe { MFCreateAttributes(&mut attributes, 1)? };
        Ok(Self {
            attributes: attributes.ok_or_else(windows::core::Error::empty)?,
            source: Mutex::new(None),
        })
    }
}

forward_attributes!(Activator_Impl, attributes);

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        let mut guard = self.source.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(MediaSource::create()?);
        }
        let source = guard.as_ref().expect("set above");
        // SAFETY: riid/ppv come from the COM caller.
        unsafe { source.query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        if let Some(source) = self.source.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: plain COM call.
            unsafe { source.Shutdown()? };
        }
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        self.source.lock().unwrap_or_else(|e| e.into_inner()).take();
        Ok(())
    }
}
