use firewheel_core::channel_config::ChannelCount;
use firewheel_core::clock::InstantMusical;
use firewheel_core::collector::ArcGc;
use firewheel_core::diff::ParamPath;
use firewheel_core::event::{NodeEvent, NodeEventType, ParamData};
use firewheel_core::node::NodeID;
use firewheel_graph::backend::{AudioBackend, SimpleDeviceConfig, SimpleStreamConfig};
use firewheel_graph::processor::ContextToProcessorMsg;
use firewheel_graph::{FirewheelConfig, FirewheelCtx};
use firewheel_nodes::{
    beep_test::BeepTestNode, sampler::SamplerNode, svf::SvfStereoNode, volume::VolumeNode,
    volume_pan::VolumePanNode,
};
use lazy_static::lazy_static;
use symphonium::SymphoniumLoader;

use std::ffi::{c_char, CStr, CString};
use std::os::raw::c_int;
use std::ptr;
use std::sync::{Arc, Mutex};

mod internal {
    #[cfg(not(any(feature = "cpal", feature = "rtaudio")))]
    pub use super::no_backend::NoBackend as Backend;
    #[cfg(feature = "cpal")]
    pub use firewheel_cpal::CpalBackend as Backend;
    #[cfg(all(feature = "rtaudio", not(feature = "cpal")))]
    pub use firewheel_rtaudio::RtAudioBackend as Backend;
}
use internal::Backend;

lazy_static! {
    static ref LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);
}

fn set_last_error(err: String) {
    if let Ok(mut last_error) = LAST_ERROR.lock() {
        *last_error = Some(CString::new(err).unwrap_or_default());
    }
}

fn fail<T: Into<String>>(msg: T) -> c_int {
    set_last_error(msg.into());
    -1
}

#[no_mangle]
pub extern "C" fn fw_get_last_error() -> *const c_char {
    if let Ok(mut last_error) = LAST_ERROR.lock() {
        if let Some(err) = last_error.take() {
            return err.into_raw();
        }
    }
    ptr::null()
}

/// Opaque struct for a loaded sample
#[repr(C)]
pub struct FwSample {
    _private: [u8; 0],
}

#[cfg(not(any(feature = "cpal", feature = "rtaudio")))]
mod no_backend {
    use firewheel_graph::backend::{AudioBackend, DeviceInfoSimple, SimpleStreamConfig};
    use firewheel_graph::processor::FirewheelProcessor;
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::time::Duration;

    #[derive(Debug)]
    pub struct NoBackendError;

    impl Display for NoBackendError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "No Backend")
        }
    }

    impl Error for NoBackendError {}

    pub struct NoBackend;

    impl AudioBackend for NoBackend {
        type Enumerator = Self;
        type Config = SimpleStreamConfig;
        type StartStreamError = NoBackendError;
        type StreamError = NoBackendError;
        type Instant = std::time::Instant;

        fn enumerator() -> Self::Enumerator {
            Self
        }

        fn input_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
            Vec::new()
        }

        fn output_devices_simple(&mut self) -> Vec<DeviceInfoSimple> {
            Vec::new()
        }

        fn convert_simple_config(&mut self, config: &SimpleStreamConfig) -> Self::Config {
            config.clone()
        }

        fn start_stream(
            _config: Self::Config,
        ) -> Result<(Self, firewheel_core::StreamInfo), Self::StartStreamError> {
            unimplemented!()
        }

        fn set_processor(&mut self, _processor: FirewheelProcessor<Self>) {
            unimplemented!()
        }

        fn poll_status(&mut self) -> Result<(), Self::StreamError> {
            Ok(())
        }

        fn delay_from_last_process(&self, _process_timestamp: Self::Instant) -> Option<Duration> {
            None
        }
    }
}

/// An enumeration of the available audio backends.
#[repr(u32)]
pub enum FwBackend {
    FwBackendCpal,
    FwBackendRtAudio,
    FwBackendNone,
}

/// Opaque struct for FirewheelConfig.
#[repr(C)]
pub struct FwConfig {
    _private: [u8; 0],
}

/// Create a new Firewheel config with default values.
#[no_mangle]
pub extern "C" fn fw_config_create() -> *mut FwConfig {
    let config = Box::new(FirewheelConfig::default());
    Box::into_raw(config) as *mut FwConfig
}

/// Free a Firewheel config.
#[no_mangle]
pub unsafe extern "C" fn fw_config_free(config: *mut FwConfig) {
    if !config.is_null() {
        let _ = (*(config as *mut FirewheelConfig)).clone();
    }
}

/// Set the number of graph inputs in a Firewheel config.
#[no_mangle]
pub unsafe extern "C" fn fw_config_set_num_graph_inputs(config: *mut FwConfig, value: u32) {
    if config.is_null() {
        return;
    }
    let config = &mut *(config as *mut FirewheelConfig);
    config.num_graph_inputs = ChannelCount::new(value).unwrap_or(ChannelCount::ZERO);
}

/// Set the number of graph outputs in a Firewheel config.
#[no_mangle]
pub unsafe extern "C" fn fw_config_set_num_graph_outputs(config: *mut FwConfig, value: u32) {
    if config.is_null() {
        return;
    }
    let config = &mut *(config as *mut FirewheelConfig);
    config.num_graph_outputs = ChannelCount::new(value).unwrap_or(ChannelCount::ZERO);
}

/// Set whether outputs should be hard clipped in a Firewheel config.
#[no_mangle]
pub unsafe extern "C" fn fw_config_set_hard_clip_outputs(config: *mut FwConfig, value: c_int) {
    if config.is_null() {
        return;
    }
    let config = &mut *(config as *mut FirewheelConfig);
    config.hard_clip_outputs = value != 0;
}

/// Set the declick seconds in a Firewheel config.
#[no_mangle]
pub unsafe extern "C" fn fw_config_set_declick_seconds(config: *mut FwConfig, value: f32) {
    if config.is_null() {
        return;
    }
    let config = &mut *(config as *mut FirewheelConfig);
    config.declick_seconds = value;
}

/// Opaque struct for the audio context.
#[repr(C)]
pub struct FwContext {
    _private: [u8; 0],
}

/// Create a new audio context with the given config.
/// If `config` is NULL, a default config will be used.
#[no_mangle]
pub extern "C" fn fw_context_create(config: *mut FwConfig) -> *mut FwContext {
    let rust_config = if config.is_null() {
        FirewheelConfig::default()
    } else {
        // SAFETY: `config` is checked for null and assumed to be a valid `FwConfig`.
        unsafe { (*(config as *mut FirewheelConfig)).clone() }
    };

    let ctx = Box::new(FirewheelCtx::<Backend>::new(rust_config));
    Box::into_raw(ctx) as *mut FwContext
}

/// Free an audio context.
#[no_mangle]
pub unsafe extern "C" fn fw_context_free(ctx: *mut FwContext) {
    if !ctx.is_null() {
        // SAFETY: `ctx` is checked for null and assumed to be a valid `FirewheelCtx`.
        // The type `FirewheelCtx<Backend>` is used here, relying on the same conditional
        // compilation as `fw_context_create`.
        let _ = Box::from_raw(ctx as *mut FirewheelCtx<Backend>);
    }
}

/// Set the playing state of the context's musical transport.
#[no_mangle]
pub extern "C" fn fw_context_set_playing(ctx: *mut FwContext, playing: bool) {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        #[cfg(feature = "musical_transport")]
        {
            let mut ts = ctx.transport_state().clone();
            *ts.playing.as_mut_unsync() = playing;
            let _ = ctx.sync_transport(&ts);
        }
        #[cfg(not(feature = "musical_transport"))]
        {
            // TODO: Log a warning if musical_transport feature is not enabled
        }
    }
}

/// Get the playing state of the context's musical transport.
/// Returns true if playing, false otherwise.
#[no_mangle]
pub extern "C" fn fw_context_is_playing(ctx: *mut FwContext) -> bool {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_ref() } {
        #[cfg(feature = "musical_transport")]
        {
            let playing = ctx.transport_state().playing.as_ref();
            return *playing;
        }
    }
    false
}

/// Set the static beats per minute (BPM) of the context's musical transport.
/// If bpm is 0.0 or negative, the static transport will be unset.
#[no_mangle]
pub extern "C" fn fw_context_set_static_beats_per_minute(ctx: *mut FwContext, bpm: f64) {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        #[cfg(feature = "musical_transport")]
        {
            let mut ts = ctx.transport_state().clone();
            if bpm > 0.0 {
                ts.set_static_transport(Some(bpm));
            } else {
                ts.set_static_transport(None);
            }
            let _ = ctx.sync_transport(&ts);
        }
        #[cfg(not(feature = "musical_transport"))]
        {
            // TODO: Log a warning if musical_transport feature is not enabled
        }
    }
}

/// Get the static beats per minute (BPM) of the context's musical transport.
/// Returns 0.0 if no static transport is set or if musical_transport feature is not enabled.
#[no_mangle]
pub extern "C" fn fw_context_get_beats_per_minute(ctx: *mut FwContext) -> f64 {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_ref() } {
        #[cfg(feature = "musical_transport")]
        {
            return ctx.transport_state().beats_per_minute().unwrap_or(0.0);
        }
    }
    0.0
}

/// Set the musical playhead of the context's musical transport.
#[no_mangle]
pub extern "C" fn fw_context_set_playhead(ctx: *mut FwContext, playhead_musical: f64) {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        #[cfg(feature = "musical_transport")]
        {
            let mut ts = ctx.transport_state().clone();
            *ts.playhead.as_mut_unsync() = InstantMusical(playhead_musical);
            let _ = ctx.sync_transport(&ts);
        }
        #[cfg(not(feature = "musical_transport"))]
        {
            // TODO: Log a warning if musical_transport feature is not enabled
        }
    }
}

/// Get the musical playhead of the context's musical transport.
/// Returns 0.0 if musical_transport feature is not enabled.
#[no_mangle]
pub extern "C" fn fw_context_get_playhead(ctx: *mut FwContext) -> f64 {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_ref() } {
        #[cfg(feature = "musical_transport")]
        {
            return ctx.transport_state().playhead.as_ref().0;
        }
    }
    0.0
}

/// Set the speed multiplier of the context's musical transport.
#[no_mangle]
pub extern "C" fn fw_context_set_speed_multiplier(ctx: *mut FwContext, multiplier: f64) {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        #[cfg(feature = "musical_transport")]
        {
            let mut ts = ctx.transport_state().clone();
            // TODO: expose change_at
            ts.set_speed_multiplier(multiplier, None);
            let _ = ctx.sync_transport(&ts);
        }
        #[cfg(not(feature = "musical_transport"))]
        {
            // TODO: Log a warning if musical_transport feature is not enabled
        }
    }
}

/// An enumeration of the built-in factory nodes.
#[repr(u32)]
pub enum FwFactoryNode {
    BeepTest,
    Sampler,
    SVF,
    Volume,
    VolumePan,
}

/// Remove a node from the Firewheel context.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn fw_node_remove(ctx: *mut FwContext, node_id: u32) -> c_int {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        // TODO: Handle the GraphError result more gracefully?..
        match ctx.remove_node(NodeID::from_bits(node_id)) {
            Ok(_) => 0,
            Err(e) => fail(format!("Failed to remove node: {:?}", e)),
        }
    } else {
        fail("Context is null")
    }
}

/// Add a new factory node to the Firewheel context.
/// Returns the ID of the new node on success, or 0 on error.
#[no_mangle]
pub extern "C" fn fw_node_add(ctx: *mut FwContext, node_type: FwFactoryNode) -> u32 {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        // TODO: expose node config
        let node_id = match node_type {
            FwFactoryNode::BeepTest => ctx.add_node(BeepTestNode::default(), None),
            FwFactoryNode::Sampler => ctx.add_node(SamplerNode::default(), None),
            FwFactoryNode::SVF => ctx.add_node(SvfStereoNode::default(), None),
            FwFactoryNode::Volume => ctx.add_node(VolumeNode::default(), None),
            FwFactoryNode::VolumePan => ctx.add_node(VolumePanNode::default(), None),
        };

        return node_id.to_bits();
    }
    0
}

/// Connects two nodes in the Firewheel context.
///
/// `src_ports` and `dst_ports` are arrays of port indices.
/// The length of both arrays must be `num_ports`.
///
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn fw_node_connect(
    ctx: *mut FwContext,
    src_node: u32,
    dst_node: u32,
    src_ports: *const u32,
    dst_ports: *const u32,
    num_ports: usize,
) -> c_int {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        // TODO: avoid allocating Vec here?..
        let mut ports_src_dst = Vec::with_capacity(num_ports);
        for i in 0..num_ports {
            let src_port = unsafe { *src_ports.add(i) };
            let dst_port = unsafe { *dst_ports.add(i) };
            ports_src_dst.push((src_port, dst_port));
        }

        // TODO: The check_for_cycles parameter should probably be exposed to the C API as well.
        match ctx.connect(
            NodeID::from_bits(src_node),
            NodeID::from_bits(dst_node),
            &ports_src_dst,
            true,
        ) {
            Ok(_) => 0,
            Err(e) => fail(format!("Failed to connect nodes: {:?}", e)),
        }
    } else {
        fail("Context is null")
    }
}

/// Set an f32 parameter on a node.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn fw_node_set_f32_parameter(
    ctx: *mut FwContext,
    node_id: u32,
    path_indices: *const u32,
    path_len: usize,
    value: f32,
) -> c_int {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        let path = if path_len == 1 {
            ParamPath::Single(unsafe { *path_indices })
        } else if path_len > 1 {
            let slice = unsafe { std::slice::from_raw_parts(path_indices, path_len) };
            ParamPath::Multi(ArcGc::new_unsized(|| Arc::<[u32]>::from(slice)))
        } else {
            return fail("Empty path is invalid for parameter setting");
        };

        let node_event = NodeEvent::new(
            NodeID::from_bits(node_id),
            NodeEventType::Param {
                data: ParamData::F32(value),
                path,
            },
        );
        let msg = ContextToProcessorMsg::EventGroup(vec![node_event]);

        ctx.send_message_to_processor(msg)
            .map(|_| 0)
            .unwrap_or(fail("Failed to send event to processor"))
    } else {
        fail("Context is null")
    }
}

/// Set a u32 parameter on a node.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn fw_node_set_u32_parameter(
    ctx: *mut FwContext,
    node_id: u32,
    path_indices: *const u32,
    path_len: usize,
    value: u32,
) -> c_int {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        let path = if path_len == 1 {
            ParamPath::Single(unsafe { *path_indices })
        } else if path_len > 1 {
            let slice = unsafe { std::slice::from_raw_parts(path_indices, path_len) };
            ParamPath::Multi(ArcGc::new_unsized(|| Arc::<[u32]>::from(slice)))
        } else {
            return fail("Empty path is invalid for parameter setting");
        };

        let node_event = NodeEvent::new(
            NodeID::from_bits(node_id),
            NodeEventType::Param {
                data: ParamData::U32(value),
                path,
            },
        );
        let msg = ContextToProcessorMsg::EventGroup(vec![node_event]);

        // TODO: Handle the error gracefully
        if ctx.send_message_to_processor(msg).is_ok() {
            0
        } else {
            fail("Failed to send event to processor")
        }
    } else {
        fail("Context is null")
    }
}

/// Update the Firewheel context, processing any pending events.
#[no_mangle]
pub extern "C" fn fw_context_update(ctx: *mut FwContext) {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        let _ = ctx.update();
    }
}

/// Disconnects two nodes in the Firewheel context.
///
/// `src_ports` and `dst_ports` are arrays of port indices.
/// The length of both arrays must be `num_ports`.
#[no_mangle]

pub extern "C" fn fw_node_disconnect(
    ctx: *mut FwContext,
    src_node: u32,
    dst_node: u32,
    src_ports: *const u32,
    dst_ports: *const u32,
    num_ports: usize,
) -> c_int {
    if let Some(ctx) = unsafe { (ctx as *mut FirewheelCtx<Backend>).as_mut() } {
        let mut ports_src_dst = Vec::with_capacity(num_ports);

        for i in 0..num_ports {
            let src_port = unsafe { *src_ports.add(i) };
            let dst_port = unsafe { *dst_ports.add(i) };
            ports_src_dst.push((src_port, dst_port));
        }

        match ctx.disconnect(
            NodeID::from_bits(src_node),
            NodeID::from_bits(dst_node),
            &ports_src_dst,
        ) {
            true => 0,
            false => fail("Failed to disconnect nodes"),
        }
    } else {
        fail("Context is null")
    }
}

/// Load a sample from a file path.
/// Returns a pointer to an FwSample on success, or NULL on error.
/// Use fw_get_last_error to retrieve the error message.
#[no_mangle]
pub extern "C" fn fw_sample_load_from_file(path: *const c_char) -> *mut FwSample {
    let c_str = unsafe {
        if path.is_null() {
            set_last_error("Invalid path".to_string());
            return ptr::null_mut();
        }

        CStr::from_ptr(path)
    };

    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_last_error("Invalid UTF-8 in path".to_string());
            return ptr::null_mut();
        }
    };

    match SymphoniumLoader::new().load(path_str, None, Default::default(), None) {
        Ok(loader) => {
            let sample = Box::new(loader);
            Box::into_raw(sample) as *mut FwSample
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Free a loaded sample.
#[no_mangle]
pub unsafe extern "C" fn fw_sample_free(sample: *mut FwSample) {
    if !sample.is_null() {
        let _ = Box::from_raw(sample as *mut SymphoniumLoader);
    }
}

/// Opaque struct for a list of audio devices.
#[repr(C)]
pub struct FwAudioDeviceList {
    _private: [u8; 0],
}

/// Opaque struct for stream configuration.
#[repr(C)]
pub struct FwStreamConfig {
    _private: [u8; 0],
}

/// An enumeration of the fields that can be set in a stream configuration.
#[repr(u32)]
pub enum FwStreamConfigField {
    Input,
    Output,
    SampleRate,
}

/// A union of the possible values for a stream configuration field.
#[repr(C)]
pub union FwStreamConfigValue {
    pub device_name: *const c_char,
    pub sample_rate: u32,
    pub num_channels: u32,
}

/// Create a new stream configuration with default values.
#[no_mangle]
pub extern "C" fn fw_stream_config_create() -> *mut FwStreamConfig {
    let config = Box::new(SimpleStreamConfig::default());
    Box::into_raw(config) as *mut FwStreamConfig
}

/// Free a stream configuration.
#[no_mangle]
pub unsafe extern "C" fn fw_stream_config_free(config: *mut FwStreamConfig) {
    if !config.is_null() {
        let _ = (*(config as *mut SimpleStreamConfig)).clone();
    }
}

/// Set a field in a stream configuration.
#[no_mangle]
pub unsafe extern "C" fn fw_stream_config_set(
    config: *mut FwStreamConfig,
    field: FwStreamConfigField,
    value: FwStreamConfigValue,
) {
    if config.is_null() {
        return;
    }
    let config = &mut *(config as *mut SimpleStreamConfig);
    match field {
        FwStreamConfigField::Input => {
            config.input = if value.device_name.is_null() {
                None
            } else {
                Some(SimpleDeviceConfig {
                    device: Some(
                        CStr::from_ptr(value.device_name)
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    channels: Some(value.num_channels as usize),
                })
            };
        }
        FwStreamConfigField::Output => {
            config.output = SimpleDeviceConfig {
                device: Some(
                    CStr::from_ptr(value.device_name)
                        .to_string_lossy()
                        .into_owned(),
                ),
                channels: Some(value.num_channels as usize),
            };
        }
        FwStreamConfigField::SampleRate => {
            config.desired_sample_rate = Some(value.sample_rate);
        }
    }
}

/// Create a list of available audio devices.
#[no_mangle]
pub extern "C" fn fw_audio_device_list_create(input: bool) -> *mut FwAudioDeviceList {
    let enumerator = Backend::enumerator();
    #[cfg(feature = "cpal")]
    let devices: Vec<String> = if input {
        enumerator
            .default_host()
            .input_devices()
            .into_iter()
            .map(|d| d.name.unwrap_or_default())
            .collect()
    } else {
        enumerator
            .default_host()
            .output_devices()
            .into_iter()
            .map(|d| d.name.unwrap_or_default())
            .collect()
    };
    #[cfg(feature = "rtaudio")]
    let devices: Vec<String> = if input {
        enumerator
            .default_api()
            .iter_input_devices()
            .map(|d| d.id.name.clone())
            .collect()
    } else {
        enumerator
            .default_api()
            .iter_output_devices()
            .map(|d| d.id.name.clone())
            .collect()
    };
    let devices: Vec<CString> = devices
        .into_iter()
        .map(|d| CString::new(d).unwrap_or_default())
        .collect();
    let list = Box::new(devices);
    Box::into_raw(list) as *mut FwAudioDeviceList
}

/// Free a list of audio devices.
#[no_mangle]
pub unsafe extern "C" fn fw_audio_device_list_free(list: *mut FwAudioDeviceList) {
    if !list.is_null() {
        let _ = Box::from_raw(list as *mut Vec<CString>);
    }
}

/// Get the number of audio devices in a list.
#[no_mangle]
pub unsafe extern "C" fn fw_audio_device_list_get_len(list: *mut FwAudioDeviceList) -> usize {
    (list as *mut Vec<CString>)
        .as_ref()
        .map(|l| l.len())
        .unwrap_or(0)
}

/// Get the name of an audio device from a list
#[no_mangle]
pub unsafe extern "C" fn fw_audio_device_list_get_name(
    list: *mut FwAudioDeviceList,
    index: usize,
) -> *const c_char {
    (list as *mut Vec<CString>)
        .as_ref()
        .and_then(|l| l.get(index))
        .map(|s| s.as_ptr())
        .unwrap_or(ptr::null())
}
