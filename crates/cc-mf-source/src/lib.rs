//! Media Foundation virtual camera source for Windows 11.
//!
//! Built as a COM in-process server (`cc_mf_source.dll`). The Camera Center
//! app registers a session-lifetime virtual camera with
//! `MFCreateVirtualCamera(..., SOURCE_CLSID)`; when a video app opens it, the
//! Windows Frame Server loads this DLL, asks the class factory for an
//! `IMFActivate` and activates the [`source::MediaSource`]. Frames come from
//! the app through the shared memory described in `cc-vcam-protocol`.
//!
//! Register once from an elevated prompt: `regsvr32 cc_mf_source.dll`.
//!
//! Status: compile-checked only; needs validation on Windows 11.

#![cfg(windows)]
#![allow(non_snake_case)]

mod activator;
mod attributes;
mod shm;
mod source;
mod stream;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, ERROR_SUCCESS, HINSTANCE, HMODULE,
    S_FALSE, S_OK,
};
use windows::Win32::Media::MediaFoundation::IMFActivate;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::core::{
    BOOL, GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, Ref, Result, implement,
};

pub const CLSID_SOURCE: GUID = GUID::from_u128(cc_vcam_protocol::SOURCE_CLSID_U128);

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    BOOL(1)
}

#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: ppv checked non-null above.
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activator: IMFActivate = activator::Activator::new()?.into();
        // SAFETY: riid/ppv come straight from the COM caller.
        unsafe { activator.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// # Safety
/// Called by COM with valid pointers.
#[unsafe(no_mangle)]
unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() {
        return E_POINTER;
    }
    // SAFETY: pointers validated above.
    unsafe {
        *ppv = std::ptr::null_mut();
        if *rclsid != CLSID_SOURCE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory.into();
        factory.query(riid, ppv)
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Objects may be alive in the Frame Server; never unload.
    S_FALSE
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    match register() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let key = HSTRING::from(clsid_key());
    // SAFETY: valid predefined key and string.
    let rc = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &key) };
    if rc == ERROR_SUCCESS {
        S_OK
    } else {
        rc.to_hresult()
    }
}

fn clsid_key() -> String {
    format!(
        "Software\\Classes\\CLSID\\{}",
        cc_vcam_protocol::SOURCE_CLSID_STR
    )
}

fn module_path() -> Result<Vec<u16>> {
    let mut buf = vec![0u16; 32768];
    let module = HMODULE(MODULE.load(Ordering::Relaxed));
    // SAFETY: buffer is valid for its length.
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 {
        return Err(windows::core::Error::from_thread());
    }
    buf.truncate(len);
    buf.push(0);
    Ok(buf)
}

fn set_string(key: HKEY, name: Option<&str>, value: &[u16]) -> Result<()> {
    let name = name.map(HSTRING::from);
    let name_ptr = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    // SAFETY: value is a NUL-terminated UTF-16 buffer.
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
    // SAFETY: valid open key.
    unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes)) }.ok()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_key(path: &str) -> Result<HKEY> {
    let mut key = HKEY::default();
    let path = HSTRING::from(path);
    // SAFETY: out pointer is valid.
    unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            &path,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    }
    .ok()?;
    Ok(key)
}

fn register() -> Result<()> {
    let path = module_path()?;
    let clsid = create_key(&clsid_key())?;
    let result = set_string(clsid, None, &wide("Camera Center Virtual Camera Source"));
    // SAFETY: key opened above.
    unsafe {
        let _ = RegCloseKey(clsid);
    }
    result?;
    let server = create_key(&format!("{}\\InprocServer32", clsid_key()))?;
    let result = set_string(server, None, &path)
        .and_then(|()| set_string(server, Some("ThreadingModel"), &wide("Both")));
    // SAFETY: key opened above.
    unsafe {
        let _ = RegCloseKey(server);
    }
    result
}
