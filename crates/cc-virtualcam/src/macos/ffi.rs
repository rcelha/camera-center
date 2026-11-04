//! Minimal hand-written bindings for the CoreMediaIO / CoreMedia / CoreVideo /
//! CoreFoundation C APIs used by the sink client.
#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;

pub type OSStatus = i32;
pub type CVReturn = i32;
pub type Boolean = u8;
pub type CMIOObjectID = u32;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CMSimpleQueueRef = *mut c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CVPixelBufferPoolRef = *mut c_void;
pub type CMFormatDescriptionRef = *mut c_void;
pub type CMSampleBufferRef = *mut c_void;
pub type CMClockRef = *mut c_void;

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

pub const kCMIOObjectSystemObject: CMIOObjectID = 1;
pub const kCMIOObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kCMIOObjectPropertyElementMain: u32 = 0;
pub const kCMIOHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kCMIODevicePropertyDeviceUID: u32 = fourcc(b"uid ");
pub const kCMIODevicePropertyStreams: u32 = fourcc(b"stm#");
pub const kCMIOObjectPropertyName: u32 = fourcc(b"lnam");
pub const kCVPixelFormatType_32BGRA: u32 = fourcc(b"BGRA");
pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt32Type: isize = 3;

#[repr(C)]
pub struct CMIOObjectPropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

impl CMIOObjectPropertyAddress {
    pub fn global(selector: u32) -> Self {
        Self {
            selector,
            scope: kCMIOObjectPropertyScopeGlobal,
            element: kCMIOObjectPropertyElementMain,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

pub const kCMTimeFlags_Valid: u32 = 1;

impl CMTime {
    pub const INVALID: CMTime = CMTime {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    };

    pub fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: kCMTimeFlags_Valid,
            epoch: 0,
        }
    }
}

#[repr(C)]
pub struct CMSampleTimingInfo {
    pub duration: CMTime,
    pub presentationTimeStamp: CMTime,
    pub decodeTimeStamp: CMTime,
}

#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _opaque: [usize; 6],
}

#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _opaque: [usize; 5],
}

pub type CMIOStreamQueueAlteredProc =
    Option<unsafe extern "C" fn(stream: CMIOObjectID, token: *mut c_void, refcon: *mut c_void)>;

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    pub fn CMIOObjectGetPropertyDataSize(
        object: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
    ) -> OSStatus;
    pub fn CMIOObjectGetPropertyData(
        object: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data_used: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    pub fn CMIOStreamCopyBufferQueue(
        stream: CMIOObjectID,
        queue_altered: CMIOStreamQueueAlteredProc,
        refcon: *mut c_void,
        queue: *mut CMSimpleQueueRef,
    ) -> OSStatus;
    pub fn CMIODeviceStartStream(device: CMIOObjectID, stream: CMIOObjectID) -> OSStatus;
    pub fn CMIODeviceStopStream(device: CMIOObjectID, stream: CMIOObjectID) -> OSStatus;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    pub fn CMSimpleQueueEnqueue(queue: CMSimpleQueueRef, element: *const c_void) -> OSStatus;
    pub fn CMSimpleQueueGetCount(queue: CMSimpleQueueRef) -> i32;
    pub fn CMSimpleQueueGetCapacity(queue: CMSimpleQueueRef) -> i32;
    pub fn CMVideoFormatDescriptionCreateForImageBuffer(
        allocator: CFAllocatorRef,
        image_buffer: CVPixelBufferRef,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    pub fn CMSampleBufferCreateForImageBuffer(
        allocator: CFAllocatorRef,
        image_buffer: CVPixelBufferRef,
        data_ready: Boolean,
        make_data_ready_callback: *const c_void,
        make_data_ready_refcon: *mut c_void,
        format_description: CMFormatDescriptionRef,
        timing: *const CMSampleTimingInfo,
        out: *mut CMSampleBufferRef,
    ) -> OSStatus;
    pub fn CMClockGetHostTimeClock() -> CMClockRef;
    pub fn CMClockGetTime(clock: CMClockRef) -> CMTime;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    pub static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    pub static kCVPixelBufferWidthKey: CFStringRef;
    pub static kCVPixelBufferHeightKey: CFStringRef;
    pub static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;

    pub fn CVPixelBufferPoolCreate(
        allocator: CFAllocatorRef,
        pool_attributes: CFDictionaryRef,
        pixel_buffer_attributes: CFDictionaryRef,
        out: *mut CVPixelBufferPoolRef,
    ) -> CVReturn;
    pub fn CVPixelBufferPoolCreatePixelBuffer(
        allocator: CFAllocatorRef,
        pool: CVPixelBufferPoolRef,
        out: *mut CVPixelBufferRef,
    ) -> CVReturn;
    pub fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> CVReturn;
    pub fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> CVReturn;
    pub fn CVPixelBufferGetBaseAddress(buffer: CVPixelBufferRef) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(buffer: CVPixelBufferRef) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;

    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFDictionaryRef;
    pub fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: isize,
        value_ptr: *const c_void,
    ) -> CFTypeRef;
    pub fn CFStringGetCString(
        s: CFStringRef,
        buffer: *mut i8,
        buffer_size: isize,
        encoding: u32,
    ) -> Boolean;
}

/// Converts a CFString to a Rust string (does not release it).
///
/// # Safety
/// `s` must be a valid CFStringRef.
pub unsafe fn cfstring_to_string(s: CFStringRef) -> Option<String> {
    let mut buf = [0i8; 512];
    // SAFETY: caller guarantees `s` is valid; buffer length is passed.
    let ok = unsafe {
        CFStringGetCString(
            s,
            buf.as_mut_ptr(),
            buf.len() as isize,
            kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString NUL-terminates on success.
    let c = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(c.to_string_lossy().into_owned())
}

/// Reads an array-valued CMIO property of `u32` object IDs.
pub fn object_ids(object: CMIOObjectID, selector: u32) -> Vec<CMIOObjectID> {
    let addr = CMIOObjectPropertyAddress::global(selector);
    let mut size = 0u32;
    // SAFETY: addr and size are valid for the duration of the call.
    let rc =
        unsafe { CMIOObjectGetPropertyDataSize(object, &addr, 0, std::ptr::null(), &mut size) };
    if rc != 0 || size == 0 {
        return Vec::new();
    }
    let count = size as usize / std::mem::size_of::<CMIOObjectID>();
    let mut ids = vec![0u32; count];
    let mut used = 0u32;
    // SAFETY: `ids` has room for `size` bytes.
    let rc = unsafe {
        CMIOObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            size,
            &mut used,
            ids.as_mut_ptr().cast(),
        )
    };
    if rc != 0 {
        return Vec::new();
    }
    ids.truncate(used as usize / std::mem::size_of::<CMIOObjectID>());
    ids
}

/// Reads a CFString-valued CMIO property.
pub fn string_property(object: CMIOObjectID, selector: u32) -> Option<String> {
    let addr = CMIOObjectPropertyAddress::global(selector);
    let mut value: CFStringRef = std::ptr::null();
    let mut used = 0u32;
    // SAFETY: `value` is a pointer-sized out parameter.
    let rc = unsafe {
        CMIOObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            std::mem::size_of::<CFStringRef>() as u32,
            &mut used,
            (&mut value as *mut CFStringRef).cast(),
        )
    };
    if rc != 0 || value.is_null() {
        return None;
    }
    // SAFETY: the property getter returns a +1 retained CFString.
    let s = unsafe { cfstring_to_string(value) };
    unsafe { CFRelease(value) };
    s
}
