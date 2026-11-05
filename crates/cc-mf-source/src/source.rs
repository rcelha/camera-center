use std::sync::Mutex;

use windows::Win32::Foundation::{E_INVALIDARG, ERROR_SET_NOT_FOUND, S_OK};
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{ComObject, GUID, HRESULT, IUnknown, Interface, Ref, Result, implement};

use crate::stream::MediaStream;

struct State {
    queue: IMFMediaEventQueue,
    descriptor: IMFPresentationDescriptor,
    stream: Option<ComObject<MediaStream>>,
    shutdown: bool,
}

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
pub struct MediaSource {
    attributes: IMFAttributes,
    state: Mutex<State>,
}

pub(crate) fn not_found() -> windows::core::Error {
    HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into()
}

impl MediaSource {
    pub fn create() -> Result<IMFMediaSource> {
        // SAFETY: plain MF factory calls with valid arguments.
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.ok_or_else(windows::core::Error::empty)?;

            let (stream_descriptor, stream) = MediaStream::create(0)?;
            let descriptor = MFCreatePresentationDescriptor(Some(&[Some(stream_descriptor)]))?;
            descriptor.SelectStream(0)?;

            let source = ComObject::new(MediaSource {
                attributes,
                state: Mutex::new(State {
                    queue: MFCreateEventQueue()?,
                    descriptor,
                    stream: Some(stream.clone()),
                    shutdown: false,
                }),
            });
            let ex: IMFMediaSourceEx = source.to_interface();
            let interface: IMFMediaSource = ex.cast()?;
            stream.set_source(Some(interface.clone()));
            Ok(interface)
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Err(MF_E_SHUTDOWN.into());
        }
        f(&mut state)
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.with_state(|s| Ok(s.queue.clone()))
    }
}

impl IMFMediaEventGenerator_Impl for MediaSource_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // Don't hold the lock: GetEvent may block.
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

impl IMFMediaSource_Impl for MediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        self.with_state(|_| Ok(MFMEDIASOURCE_IS_LIVE.0 as u32))
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.with_state(|s| unsafe { s.descriptor.Clone() })
    }

    fn Start(
        &self,
        descriptor: Ref<IMFPresentationDescriptor>,
        time_format: *const GUID,
        _start: *const PROPVARIANT,
    ) -> Result<()> {
        let descriptor = descriptor.ok()?;
        if !time_format.is_null() && unsafe { *time_format } != GUID::zeroed() {
            return Err(MF_E_UNSUPPORTED_TIME_FORMAT.into());
        }
        self.with_state(|s| unsafe {
            let count = descriptor.GetStreamDescriptorCount()?;
            for i in 0..count {
                let mut selected = windows::core::BOOL::default();
                let mut sd = None;
                descriptor.GetStreamDescriptorByIndex(i, &mut selected, &mut sd)?;
                let sd = sd.ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
                if sd.GetStreamIdentifier()? != 0 {
                    return Err(E_INVALIDARG.into());
                }
                if let Some(stream) = &s.stream {
                    let active = selected.as_bool();
                    stream.set_active(active)?;
                    if active {
                        let unknown: IUnknown = stream.to_interface();
                        s.queue.QueueEventParamUnk(
                            MENewStream.0 as u32,
                            &GUID::zeroed(),
                            S_OK,
                            &unknown,
                        )?;
                        stream.start()?;
                    }
                }
            }
            s.queue.QueueEventParamVar(
                MESourceStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )
        })
    }

    fn Stop(&self) -> Result<()> {
        self.with_state(|s| unsafe {
            if let Some(stream) = &s.stream {
                stream.stop()?;
            }
            s.queue.QueueEventParamVar(
                MESourceStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )
        })
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Ok(());
        }
        state.shutdown = true;
        if let Some(stream) = state.stream.take() {
            stream.shutdown();
        }
        unsafe { state.queue.Shutdown() }
    }
}

impl IMFMediaSourceEx_Impl for MediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        self.with_state(|_| Ok(self.attributes.clone()))
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        self.with_state(|s| match (&s.stream, id) {
            (Some(stream), 0) => Ok(stream.attributes()),
            _ => Err(E_INVALIDARG.into()),
        })
    }

    fn SetD3DManager(&self, _manager: Ref<IUnknown>) -> Result<()> {
        // We produce system-memory samples only.
        Ok(())
    }
}

impl IMFGetService_Impl for MediaSource_Impl {
    fn GetService(
        &self,
        _service: *const GUID,
        _riid: *const GUID,
        _ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for MediaSource_Impl {
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
