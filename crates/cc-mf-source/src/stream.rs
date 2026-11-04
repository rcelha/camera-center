use std::sync::Mutex;

use cc_vcam_protocol::{FPS, FRAME_SIZE, HEIGHT, WIDTH};
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::KernelStreaming::{
    IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{ComObject, GUID, HRESULT, IUnknown, Interface, Ref, Result, implement};

use crate::shm::Mapping;
use crate::source::not_found;

struct State {
    queue: IMFMediaEventQueue,
    source: Option<IMFMediaSource>,
    stream_state: MF_STREAM_STATE,
    mapping: Option<Mapping>,
    frame: Vec<u8>,
    shutdown: bool,
}

#[implement(IMFMediaStream2, IKsControl)]
pub struct MediaStream {
    descriptor: IMFStreamDescriptor,
    attributes: IMFAttributes,
    state: Mutex<State>,
}

fn media_type() -> Result<IMFMediaType> {
    // SAFETY: plain MF attribute setters.
    unsafe {
        let mt = MFCreateMediaType()?;
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        mt.SetUINT64(&MF_MT_FRAME_SIZE, ((WIDTH as u64) << 32) | HEIGHT as u64)?;
        mt.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
        mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        mt.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        mt.SetUINT32(&MF_MT_DEFAULT_STRIDE, WIDTH)?;
        mt.SetUINT32(&MF_MT_SAMPLE_SIZE, FRAME_SIZE as u32)?;
        Ok(mt)
    }
}

/// Mid-grey NV12 frame shown until the app sends frames.
fn placeholder() -> Vec<u8> {
    let luma = (WIDTH * HEIGHT) as usize;
    let mut v = vec![40u8; FRAME_SIZE];
    v[luma..].fill(128);
    v
}

fn set_stream_attributes(attrs: &IMFAttributes, id: u32) -> Result<()> {
    // SAFETY: plain MF attribute setters.
    unsafe {
        attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, id)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        attrs.SetUINT32(
            &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
            MFFrameSourceTypes_Color.0 as u32,
        )?;
    }
    Ok(())
}

impl MediaStream {
    pub fn create(id: u32) -> Result<(IMFStreamDescriptor, ComObject<MediaStream>)> {
        // SAFETY: plain MF factory calls.
        unsafe {
            let mt = media_type()?;
            let descriptor = MFCreateStreamDescriptor(id, &[Some(mt.clone())])?;
            descriptor.GetMediaTypeHandler()?.SetCurrentMediaType(&mt)?;
            set_stream_attributes(&descriptor.cast()?, id)?;

            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.ok_or_else(windows::core::Error::empty)?;
            set_stream_attributes(&attributes, id)?;

            let stream = ComObject::new(MediaStream {
                descriptor: descriptor.clone(),
                attributes,
                state: Mutex::new(State {
                    queue: MFCreateEventQueue()?,
                    source: None,
                    stream_state: MF_STREAM_STATE_STOPPED,
                    mapping: None,
                    frame: placeholder(),
                    shutdown: false,
                }),
            });
            Ok((descriptor, stream))
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn attributes(&self) -> IMFAttributes {
        self.attributes.clone()
    }

    pub fn set_source(&self, source: Option<IMFMediaSource>) {
        self.lock().source = source;
    }

    pub fn set_active(&self, active: bool) -> Result<()> {
        let mut s = self.lock();
        if !active {
            s.stream_state = MF_STREAM_STATE_STOPPED;
        }
        Ok(())
    }

    pub fn start(&self) -> Result<()> {
        let mut s = self.lock();
        if s.mapping.is_none() {
            // Failing to create the mapping only means we show the placeholder.
            s.mapping = Mapping::create().ok();
        }
        s.stream_state = MF_STREAM_STATE_RUNNING;
        // SAFETY: plain MF call.
        unsafe {
            s.queue.QueueEventParamVar(
                MEStreamStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )
        }
    }

    pub fn stop(&self) -> Result<()> {
        let mut s = self.lock();
        s.stream_state = MF_STREAM_STATE_STOPPED;
        // SAFETY: plain MF call.
        unsafe {
            s.queue.QueueEventParamVar(
                MEStreamStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )
        }
    }

    pub fn shutdown(&self) {
        let mut s = self.lock();
        s.shutdown = true;
        s.source = None;
        s.mapping = None;
        // SAFETY: plain MF call.
        unsafe {
            let _ = s.queue.Shutdown();
        }
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        let s = self.lock();
        if s.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(s.queue.clone())
    }
}

impl IMFMediaEventGenerator_Impl for MediaStream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        let queue = self.queue()?;
        unsafe { queue.GetEvent(flags.0) }
    }

    fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }

    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        let queue = self.queue()?;
        unsafe { queue.EndGetEvent(result.as_ref()) }
    }

    fn QueueEvent(
        &self,
        met: u32,
        ext: *const GUID,
        hr: HRESULT,
        value: *const PROPVARIANT,
    ) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.QueueEventParamVar(met, ext, hr, value) }
    }
}

impl IMFMediaStream_Impl for MediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        let s = self.lock();
        if s.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        s.source.clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        if self.lock().shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(self.descriptor.clone())
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        let mut s = self.lock();
        if s.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        if s.stream_state != MF_STREAM_STATE_RUNNING {
            return Err(MF_E_MEDIA_SOURCE_WRONGSTATE.into());
        }
        let State {
            mapping,
            frame,
            queue,
            ..
        } = &mut *s;
        if mapping.is_none() {
            *mapping = crate::shm::Mapping::create().ok();
        }
        if let Some(m) = mapping.as_ref() {
            // Keeps the previous frame if no new consistent frame is available.
            let _ = m.frame().read(frame);
        }
        // SAFETY: the buffer is locked while written and sized FRAME_SIZE.
        unsafe {
            let buffer = MFCreateMemoryBuffer(FRAME_SIZE as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(frame.as_ptr(), data, FRAME_SIZE);
            buffer.Unlock()?;
            buffer.SetCurrentLength(FRAME_SIZE as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            let now = MFGetSystemTime();
            sample.SetSampleTime(now)?;
            sample.SetSampleDuration(10_000_000 / FPS as i64)?;
            sample.SetUINT64(&MFSampleExtension_Timestamp, now as u64)?;
            if let Some(token) = token.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), S_OK, &sample)
        }
    }
}

impl IMFMediaStream2_Impl for MediaStream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        let running = value == MF_STREAM_STATE_RUNNING;
        let current = self.lock().stream_state;
        match (current == MF_STREAM_STATE_RUNNING, running) {
            (false, true) => self.start(),
            (true, false) => self.stop(),
            _ => Ok(()),
        }
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.lock().stream_state)
    }
}

impl IKsControl_Impl for MediaStream_Impl {
    fn KsProperty(
        &self,
        _p: *const KSIDENTIFIER,
        _pl: u32,
        _d: *mut core::ffi::c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(not_found())
    }

    fn KsMethod(
        &self,
        _m: *const KSIDENTIFIER,
        _ml: u32,
        _d: *mut core::ffi::c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(not_found())
    }

    fn KsEvent(
        &self,
        _e: *const KSIDENTIFIER,
        _el: u32,
        _d: *mut core::ffi::c_void,
        _dl: u32,
        _r: *mut u32,
    ) -> Result<()> {
        Err(not_found())
    }
}
