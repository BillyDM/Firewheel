
#![cfg(not(target_arch = "wasm32"))]

use std::ffi::CString;
use clack_host::events::event_types::NoteOnEvent;
use clack_host::prelude::*;
use clack_host::events::io::{InputEvents, OutputEvents};
use clack_host::process::StartedPluginAudioProcessor;

use firewheel::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::PatchError,
    event::{ParamData, ProcEvents},
    node::{AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers, ProcExtra, ProcInfo, ProcessStatus},
};
use firewheel::diff::EventQueue;
use clack_host::host::{SharedHandler, MainThreadHandler, AudioProcessorHandler};

use std::path::Path;
use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

fn set_dll_directory<P: AsRef<Path>>(path: P) {
    let wide: Vec<u16> = OsStr::new(path.as_ref())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetDllDirectoryW(wide.as_ptr());
    }
}


// ---- Define your node type ----

#[derive(Clone, Default)]
pub struct ClapPluginNode {
    pub path: String,
    pub enabled: bool,
}

// You need to inspect AudioNode trait in your version and implement its expected items:
impl AudioNode for ClapPluginNode {
    type Configuration = ClapPluginConfig;

    fn info(&self, _cfg: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("clap_plugin_node")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
    }

    fn construct_processor(
        &self,
        _cfg: &Self::Configuration,
        _ctx: ConstructProcessorContext<'_>,
    ) -> impl AudioNodeProcessor {
        let hardcoded_node = ClapPluginNode {
            // insert your own path: std::path::PathBuf::from(r"C:\Program Files\Common Files\CLAP\your_plugin_name.clap")
            path: std::path::PathBuf::from("assets/polly.clap")
                .to_string_lossy()
                .to_string(),
            ..self.clone()
        };
    
        let cfg = ClapPluginConfig {
            sample_rate: 48000.0,
            block_size: 1024,
        };
    
        let processor_result = ClapPluginProcessor::new(hardcoded_node, cfg);
    
        match processor_result {
            Ok(ref processor) => {
                println!("✅ ClapPluginProcessor successfully created:");
                println!(
                    "  - plugin: {}",
                    if processor.plugin.is_some() { "Some" } else { "None" }
                );
                println!(
                    "  - audio_proc: {}",
                    if processor.audio_proc.is_some() { "Some" } else { "None" }
                );
                println!("  - enabled: {}", processor.enabled);
            }
            Err(ref err) => {
                eprintln!("❌ Failed to construct CLAP plugin processor: {err} \n 
                In order to construct the plugin, \n
                both plugin location and ID must match to be registered");
            }
        }
    
        // Unwrap or panic after printing debug info
        processor_result.expect("❌ Failed to construct CLAP plugin processor \n
        In order to construct the plugin, \n
        both plugin location and ID must match to be registered")
    }
    
    
}

// Patch-type and Diff-type definitions
#[derive(Clone)]
pub enum ClapPluginPatch {
    Enabled(bool),
}

impl firewheel::diff::Patch for ClapPluginNode {
    type Patch = ClapPluginPatch;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, PatchError> {
        if let Some(b) = data.downcast_ref::<bool>() {
            Ok(ClapPluginPatch::Enabled(*b))
        } else {
            Err(PatchError::InvalidData)
        }
    }

    fn apply(&mut self, patch: ClapPluginPatch) {
        match patch {
            ClapPluginPatch::Enabled(value) => {
                self.enabled = value;
            }
        }
    }
}

impl firewheel::diff::Diff for ClapPluginNode {
    fn diff<E: EventQueue>(
        &self,
        other: &Self,
        path: firewheel::diff::PathBuilder,
        queue: &mut E,
    ) {
        if self.enabled != other.enabled {
            // ✅ Correct: param-based diff
            queue.push_param(self.enabled, path);


        }
    }
}


// ----- Configuration -----

#[derive(Clone, Default)]
pub struct ClapPluginConfig {
    pub sample_rate: f32,
    pub block_size: usize,
}

// ----- Processor -----

pub struct ClapPluginProcessor {
    plugin: Option<PluginInstance<MinimalHost>>,
    audio_proc: Option<StartedPluginAudioProcessor<MinimalHost>>,
    enabled: bool,
}

unsafe impl Send for ClapPluginProcessor {}


impl AudioNodeProcessor for ClapPluginProcessor {
    fn process(
        &mut self,
        _info: &ProcInfo,
        _buffers: ProcBuffers,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        if !self.enabled {
            return ProcessStatus::ClearAllOutputs;
        }
        let _proc = match &mut self.audio_proc {
            Some(p) => p,
            None => return ProcessStatus::ClearAllOutputs,
        };

        // Handle patches
        for patch in events.drain_patches::<ClapPluginNode>() {
            match patch {
                ClapPluginPatch::Enabled(v) => self.enabled = v,
            }
        }
        

        // TODO: convert MIDI events properly for your clack_host version

        let clap_buf: Vec<NoteOnEvent> = Vec::new();
        for _midi in events.drain() {
            // Example pseudo: get raw bytes
            // let bytes = midi.as_midi_bytes(); // depending on your version
            // match bytes[0] & 0xF0 { ... produce NoteOnEvent / NoteOffEvent }
        }

        // TODO: prepare audio buffer types accepted by proc.process (not AudioPorts directly)
        // e.g. InputAudioBuffers, OutputAudioBuffers
        // then call proc.process(&in_bufs, &mut out_bufs, &input_events, &mut output_events, None, None)

        let _input_events = InputEvents::from_buffer(&clap_buf);
        let _output_events = OutputEvents::void();

        // Pseudo:
        // let status = proc.process(&in_bufs, &mut out_bufs, &input_events, &mut output_events, None, None);

        // match status {
        //     Ok(_) => ProcessStatus::OutputsModified,
        //     Err(_) => ProcessStatus::ClearAllOutputs,
        // }

        ProcessStatus::ClearAllOutputs // placeholder
    }
}

impl ClapPluginProcessor {
    pub fn new(
        node: ClapPluginNode,
        cfg: ClapPluginConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path = std::fs::canonicalize(&node.path)
            .expect("Failed to resolve full path to plugin");
        println!("ClapPluginProcessor::new: loading from resolved path = {:?}", path.display());

        if let Some(parent) = path.parent() {
            set_dll_directory(parent);
            println!("Set CLAP directory to {:?}", parent);
        }

        // Load the plugin bundle
        let bundle = unsafe { PluginBundle::load(path.clone())? };

        // Get the factory from the bundle
        let factory = bundle.get_plugin_factory()
            .ok_or("Failed to get plugin factory")?;

        // List all plugins inside the bundle with their IDs and names
        println!("Available plugins in bundle:");
        for descriptor in factory.plugin_descriptors() {
            let id_str = descriptor.id()
                .and_then(|cstr| cstr.to_str().ok())
                .unwrap_or("Unknown ID");

            let name_str = descriptor.name()
                .and_then(|cstr| cstr.to_str().ok())
                .unwrap_or("Unknown Name");

            println!("  - ID: {}, Name: {}", id_str, name_str);
        }

        // TODO: Replace this with the exact plugin ID you want to instantiate!
        // For example, after running, pick an ID from the printed list
      //  let plugin_id_str = "polly"; // <-- Replace "polly" with your exact plugin ID string
        let plugin_id = CString::new("hqsoundz.polly")?;

//        let plugin_id = CString::new(plugin_id_str)?;

        // Create host info for the plugin
        let host_info = HostInfo::new("MyHost", "MyVendor", "http://localhost", "1.0")?;

        // Try to create plugin instance
        let mut instance = PluginInstance::<MinimalHost>::new(
            |_| MinimalShared::default(),
            |_| MinimalMainThread::default(),
            &bundle,
            &plugin_id,
            &host_info,
        )?;

        let audio_config = PluginAudioConfiguration {
            sample_rate: cfg.sample_rate as f64,
            min_frames_count: cfg.block_size as u32,
            max_frames_count: cfg.block_size as u32,
        };

        let started = {
            instance.activate(
                |_shared, _main| ClapPluginProcessor {
                    plugin: None,
                    audio_proc: None,
                    enabled: node.enabled,
                },
                audio_config,
            )?
        }
        .start_processing()?;

        Ok(ClapPluginProcessor {
            plugin: Some(instance),
            audio_proc: Some(started),
            enabled: node.enabled,
        })
    }
}


impl<'a> AudioProcessorHandler<'a> for ClapPluginProcessor {}
// ----- Minimal Host -----

#[derive(Default)]
pub struct MinimalShared;

// impl<'a> SharedHandler<'a> for MinimalShared {}
impl<'a> SharedHandler<'a> for MinimalShared {
    fn request_restart(&self) {
        println!("Host requested plugin restart.");
    }

    fn request_process(&self) {
        println!("Host requested process.");
    }

    fn request_callback(&self) {
        println!("Host requested callback.");
    }
}

#[derive(Default)]
pub struct MinimalMainThread;

impl<'a> MainThreadHandler<'a> for MinimalMainThread {}

pub struct MinimalHost;

impl HostHandlers for MinimalHost {
    type Shared<'a> = MinimalShared;
    type MainThread<'a> = MinimalMainThread;
    type AudioProcessor<'a> = ClapPluginProcessor;

    fn declare_extensions(_builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        // Only register extensions that implement ExtensionImplementation<MinimalHost>
    }
}
