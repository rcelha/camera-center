//! Creates the shared frame mapping the app writes into.

use cc_vcam_protocol::{MAPPING_NAME, MAPPING_SIZE, SharedFrame};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::{HSTRING, Result, w};

/// Full access for SYSTEM and LocalService (Frame Server), read/write for
/// interactively logged-on users (the Camera Center app). Nobody else.
const SDDL: windows::core::PCWSTR = w!("D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GRGW;;;IU)");

pub struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    frame: SharedFrame,
}

// SAFETY: the mapping is process-wide memory; access is synchronised through
// the protocol's sequence lock.
unsafe impl Send for Mapping {}

impl Mapping {
    pub fn create() -> Result<Self> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: SDDL is a valid constant string; sd is freed with LocalFree.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                SDDL,
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let name = HSTRING::from(MAPPING_NAME);
        // SAFETY: valid security attributes and name.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                0,
                MAPPING_SIZE as u32,
                &name,
            )
        };
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
        let handle = handle?;
        // SAFETY: handle refers to a MAPPING_SIZE section.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, MAPPING_SIZE) };
        if view.Value.is_null() {
            let err = windows::core::Error::from_thread();
            // SAFETY: handle is valid.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(err);
        }
        // SAFETY: view is MAPPING_SIZE bytes and page aligned.
        let frame = unsafe { SharedFrame::new(view.Value.cast()) };
        if !frame.is_valid() {
            frame.init();
        }
        Ok(Self {
            handle,
            view,
            frame,
        })
    }

    pub fn frame(&self) -> &SharedFrame {
        &self.frame
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: view and handle were created in `create`.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}
