use clack_extensions::audio_ports::{HostAudioPorts, HostAudioPortsImpl, RescanType};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_host::entry::PluginEntryError;
use clack_host::prelude::*;
use firewheel_core::channel_config::{ChannelConfig, ChannelCount};
use firewheel_core::event::ProcEvents;
use firewheel_core::node::{
    AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
    ProcBuffers, ProcExtra, ProcInfo,
};
use log::{debug, error, info, warn};
use std::ffi::{CString, NulError};
use thiserror::Error;

/// Information about this host.
fn host_info() -> HostInfo {
    HostInfo::new(
        "Firewheel Clap Plugin Node Host",
        "Firewheel",
        "https://github.com/BillyDM/Firewheel",
        env!("CARGO_PKG_VERSION"),
    )
    .unwrap()
}

/// Errors that happened during finder.
#[derive(Error, Debug)]
pub enum ClapPluginLoadError {
    #[error("Failed to load plugin")]
    LoadError(#[from] PluginEntryError),
    #[error("Plugin factory missing")]
    MissingPluginFactory,
    #[error("Plugin descriptor with ID not found")]
    IDNotFound,
    #[error("Failed to instantiate plugin")]
    InstantiationFailed(#[from] PluginInstanceError),
    #[error("Failed to parse provided ID")]
    ParseIDFailed(#[from] NulError),
}

/// A node that hosts a CLAP plugin
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClapPluginNode {
    /// Whether the node is currently enabled.
    pub enabled: bool,
}

impl ClapPluginNode {
    pub fn new(
        path: impl AsRef<std::ffi::OsStr>,
        id: impl AsRef<str>,
    ) -> Result<Self, ClapPluginLoadError> {
        // Safety: Loading an external library object file is inherently unsafe
        let entry = unsafe { PluginEntry::load(path)? };

        let plugin_factory = entry
            .get_plugin_factory()
            .ok_or_else(|| ClapPluginLoadError::MissingPluginFactory)?;

        let id = CString::new(id.as_ref())?;

        let _plugin_descriptor = plugin_factory
            .plugin_descriptors()
            .filter_map(|x| x.id())
            .find(|&plugin_id| plugin_id.eq(&id))
            .ok_or_else(|| ClapPluginLoadError::IDNotFound)?;

        let plugin_instance = PluginInstance::<FirewheelClapHost>::new(
            |_| FirewheelClapShared::default(),
            |_| FirewheelClapMain::default(),
            &entry,
            &id,
            &host_info(),
        )?;

        Ok(Self {
            enabled: true,
        })
    }
}

impl AudioNode for ClapPluginNode {
    type Configuration = EmptyConfig;

    fn info(&self, configuration: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("clap_plugin")
            .channel_config(ChannelConfig {
                // TODO: Dynamic channel count based on plugin?
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
    }

    fn construct_processor(
        &self,
        configuration: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let audio_config = PluginAudioConfiguration {
            sample_rate: f64::from(u32::from(cx.stream_info.sample_rate)),
            min_frames_count: 0,
            max_frames_count: u32::from(cx.stream_info.max_block_frames),
        };

        ClapPluginProcessor {
            enabled: false,
            // audio_processor: self.plugin_instance.activate(|_, _| (), audio_config).unwrap().start_processing().unwrap(),
        }
    }
}

pub struct ClapPluginProcessor {
    enabled: bool,

    /// The Clap plugin instance
    plugin_instance: PluginInstance<FirewheelClapHost>,

    /// The started Clap audio processor
    audio_processor: StartedPluginAudioProcessor<FirewheelClapHost>,
}

impl AudioNodeProcessor for ClapPluginProcessor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> firewheel_core::node::ProcessStatus {
        todo!()
    }
}

#[derive(Default)]
pub struct FirewheelClapShared;

// impl<'a> SharedHandler<'a> for MinimalShared {}
impl<'a> SharedHandler<'a> for FirewheelClapShared {
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

impl HostLogImpl for FirewheelClapShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        // TODO: Make this realtime safe with a MPSC ring buffer?

        // From clack cpal example:
        // Note: writing to stdout isn't realtime-safe, and should ideally be avoided.
        // This is only "good enough™" for an example.
        // A mpsc ringbuffer with support for dynamically-sized messages (`?Sized`) should be used to
        // send the logs the main thread without allocating or blocking.

        match severity {
            LogSeverity::Debug => debug!("{}", message),
            LogSeverity::Info => info!("{}", message),
            LogSeverity::Warning => warn!("{}", message),
            LogSeverity::Error => error!("{}", message),
            LogSeverity::Fatal => error!("[FATAL] {}", message),
            LogSeverity::HostMisbehaving => warn!("[HOST MISBEHAVING] {}", message),
            LogSeverity::PluginMisbehaving => warn!("[PLUGIN MISBEHAVING] {}", message),
        }
    }
}

#[derive(Default)]
pub struct FirewheelClapMain;

impl<'a> MainThreadHandler<'a> for FirewheelClapMain {}

impl HostAudioPortsImpl for FirewheelClapMain {
    fn is_rescan_flag_supported(&self, flag: RescanType) -> bool {
        false
    }

    fn rescan(&mut self, flag: RescanType) {
        // We don't support audio ports changing
    }
}

pub struct FirewheelClapHost;

impl HostHandlers for FirewheelClapHost {
    type Shared<'a> = FirewheelClapShared;
    type MainThread<'a> = FirewheelClapMain;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostLog>().register::<HostAudioPorts>();
    }
}
