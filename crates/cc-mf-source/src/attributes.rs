//! Forwards an `IMFAttributes_Impl` to an inner `IMFAttributes` store.

/// Implements `IMFAttributes_Impl` for `$ty` by forwarding every call to
/// `self.$field` through its vtable.
macro_rules! forward_attributes {
    ($ty:ty, $field:ident) => {
        impl windows::Win32::Media::MediaFoundation::IMFAttributes_Impl for $ty {
            fn GetItem(&self, key: *const GUID, value: *mut PROPVARIANT) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetItem)(a.as_raw(), key, value).ok() }
            }
            fn GetItemType(&self, key: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
                let a = &self.$field;
                let mut out = MF_ATTRIBUTE_TYPE::default();
                unsafe { (Interface::vtable(a).GetItemType)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn CompareItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<BOOL> {
                let a = &self.$field;
                let mut out = BOOL::default();
                unsafe {
                    (Interface::vtable(a).CompareItem)(a.as_raw(), key, value, &mut out).ok()?
                };
                Ok(out)
            }
            fn Compare(
                &self,
                theirs: Ref<IMFAttributes>,
                match_type: MF_ATTRIBUTES_MATCH_TYPE,
            ) -> Result<BOOL> {
                let a = &self.$field;
                let raw = theirs.as_ref().map_or(std::ptr::null_mut(), |t| t.as_raw());
                let mut out = BOOL::default();
                unsafe {
                    (Interface::vtable(a).Compare)(a.as_raw(), raw, match_type, &mut out).ok()?
                };
                Ok(out)
            }
            fn GetUINT32(&self, key: *const GUID) -> Result<u32> {
                let a = &self.$field;
                let mut out = 0;
                unsafe { (Interface::vtable(a).GetUINT32)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetUINT64(&self, key: *const GUID) -> Result<u64> {
                let a = &self.$field;
                let mut out = 0;
                unsafe { (Interface::vtable(a).GetUINT64)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetDouble(&self, key: *const GUID) -> Result<f64> {
                let a = &self.$field;
                let mut out = 0.0;
                unsafe { (Interface::vtable(a).GetDouble)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetGUID(&self, key: *const GUID) -> Result<GUID> {
                let a = &self.$field;
                let mut out = GUID::zeroed();
                unsafe { (Interface::vtable(a).GetGUID)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetStringLength(&self, key: *const GUID) -> Result<u32> {
                let a = &self.$field;
                let mut out = 0;
                unsafe { (Interface::vtable(a).GetStringLength)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetString(
                &self,
                key: *const GUID,
                value: PWSTR,
                size: u32,
                len: *mut u32,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetString)(a.as_raw(), key, value, size, len).ok() }
            }
            fn GetAllocatedString(
                &self,
                key: *const GUID,
                value: *mut PWSTR,
                len: *mut u32,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe {
                    (Interface::vtable(a).GetAllocatedString)(a.as_raw(), key, value, len).ok()
                }
            }
            fn GetBlobSize(&self, key: *const GUID) -> Result<u32> {
                let a = &self.$field;
                let mut out = 0;
                unsafe { (Interface::vtable(a).GetBlobSize)(a.as_raw(), key, &mut out).ok()? };
                Ok(out)
            }
            fn GetBlob(
                &self,
                key: *const GUID,
                buf: *mut u8,
                size: u32,
                len: *mut u32,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetBlob)(a.as_raw(), key, buf, size, len).ok() }
            }
            fn GetAllocatedBlob(
                &self,
                key: *const GUID,
                buf: *mut *mut u8,
                size: *mut u32,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetAllocatedBlob)(a.as_raw(), key, buf, size).ok() }
            }
            fn GetUnknown(
                &self,
                key: *const GUID,
                riid: *const GUID,
                ppv: *mut *mut core::ffi::c_void,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetUnknown)(a.as_raw(), key, riid, ppv).ok() }
            }
            fn SetItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetItem)(a.as_raw(), key, value).ok() }
            }
            fn DeleteItem(&self, key: *const GUID) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).DeleteItem)(a.as_raw(), key).ok() }
            }
            fn DeleteAllItems(&self) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).DeleteAllItems)(a.as_raw()).ok() }
            }
            fn SetUINT32(&self, key: *const GUID, value: u32) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetUINT32)(a.as_raw(), key, value).ok() }
            }
            fn SetUINT64(&self, key: *const GUID, value: u64) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetUINT64)(a.as_raw(), key, value).ok() }
            }
            fn SetDouble(&self, key: *const GUID, value: f64) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetDouble)(a.as_raw(), key, value).ok() }
            }
            fn SetGUID(&self, key: *const GUID, value: *const GUID) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetGUID)(a.as_raw(), key, value).ok() }
            }
            fn SetString(&self, key: *const GUID, value: &PCWSTR) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetString)(a.as_raw(), key, *value).ok() }
            }
            fn SetBlob(&self, key: *const GUID, buf: *const u8, size: u32) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).SetBlob)(a.as_raw(), key, buf, size).ok() }
            }
            fn SetUnknown(&self, key: *const GUID, unknown: Ref<IUnknown>) -> Result<()> {
                let a = &self.$field;
                let raw = unknown
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |u| u.as_raw());
                unsafe { (Interface::vtable(a).SetUnknown)(a.as_raw(), key, raw).ok() }
            }
            fn LockStore(&self) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).LockStore)(a.as_raw()).ok() }
            }
            fn UnlockStore(&self) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).UnlockStore)(a.as_raw()).ok() }
            }
            fn GetCount(&self) -> Result<u32> {
                let a = &self.$field;
                let mut out = 0;
                unsafe { (Interface::vtable(a).GetCount)(a.as_raw(), &mut out).ok()? };
                Ok(out)
            }
            fn GetItemByIndex(
                &self,
                index: u32,
                key: *mut GUID,
                value: *mut PROPVARIANT,
            ) -> Result<()> {
                let a = &self.$field;
                unsafe { (Interface::vtable(a).GetItemByIndex)(a.as_raw(), index, key, value).ok() }
            }
            fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> Result<()> {
                let a = &self.$field;
                let raw = dest.as_ref().map_or(std::ptr::null_mut(), |d| d.as_raw());
                unsafe { (Interface::vtable(a).CopyAllItems)(a.as_raw(), raw).ok() }
            }
        }
    };
}

pub(crate) use forward_attributes;
