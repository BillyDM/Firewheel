// TODO: The logic in this has become incredibly complex and error-prone. The
// sampler engine should probably be rewritten using a state machine.
//
// Some features that are currently missing include:
// * Ability to set loop start/end points
// * Better quality time/pitch shifting algorithms (and possibly an API where
//   users can implement their own resampling algorithms)
// * Ability to stream samples from a network/disk (this could be done using
//   a custom `SampleResource`).

use firewheel_core::clock::{DurationSamples, DurationSeconds};
use firewheel_core::collector::{OwnedGc, OwnedGcUnsized};
use firewheel_core::node::{NodeError, ProcBuffers, ProcExtra, ProcStreamCtx};

use bevy_platform::sync::{Arc, Mutex};
use bevy_platform::time::Instant;
use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};
use firewheel_core::diff::{EventQueue, NotifyID, PatchError, PathBuilder, RealtimeClone};
use smallvec::SmallVec;
use triple_buffer::{Input, Output};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Box;
#[cfg(not(feature = "std"))]
use num_traits::Float;

use firewheel_core::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    clock::InstantSeconds,
    collector::ArcGc,
    diff::{Diff, Notify, ParamPath, Patch},
    dsp::{
        buffer::InstanceBuffer,
        declick::{DeclickFadeCurve, Declicker},
        volume::{DEFAULT_MIN_AMP, Volume},
    },
    event::{NodeEventType, ParamData, ProcEvents},
    mask::{MaskType, SilenceMask},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcInfo,
        ProcessStatus,
    },
    sample_resource::SampleResource,
};

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

pub const MAX_OUT_CHANNELS: usize = 8;
pub const DEFAULT_NUM_DECLICKERS: usize = 2;
pub const MIN_PLAYBACK_SPEED: f64 = 0.0000001;

mod resampler;
mod resource;

pub use self::resource::{SamplerNodeResource, StreamedSample};

use self::resampler::Resampler;

pub type PlaybackID = NotifyID;

/// The configuration of a [`SamplerNode`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SamplerConfig {
    /// The number of channels in this node.
    ///
    /// By default this is set to [`NonZeroChannelCount::STEREO`].
    pub channels: NonZeroChannelCount,
    /// The maximum number of "declickers" present on this node.
    /// The more declickers there are, the more samples that can be declicked
    /// when played in rapid succession. (Note more declickers will allocate
    /// more memory).
    ///
    /// By default this is set to `2`.
    pub num_declickers: u32,
    /// The quality of the resampling algorithm used when changing the playback
    /// speed.
    pub speed_quality: PlaybackSpeedQuality,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            num_declickers: DEFAULT_NUM_DECLICKERS as u32,
            speed_quality: PlaybackSpeedQuality::default(),
        }
    }
}

/// The quality of the resampling algorithm used for changing the playback
/// speed of a sampler node.
#[non_exhaustive]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PlaybackSpeedQuality {
    #[default]
    /// Low quality, fast performance. Recommended for most use cases.
    ///
    /// More specifically, this uses a linear resampling algorithm with no
    /// antialiasing filter.
    LinearFast,
    // TODO: more quality options
}

/// A node that plays samples
///
/// It supports pausing, resuming, looping, and changing the playback speed.
#[derive(Debug, Clone, Copy, Diff, Patch, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SamplerNode {
    /// The volume to play the sample at.
    ///
    /// Note, this gain parameter is *NOT* smoothed! If you need the gain to be
    /// smoothed, please use a [`VolumeNode`] or a [`VolumePanNode`].
    ///
    /// [`VolumeNode`]: crate::volume::VolumeNode
    /// [`VolumePanNode`]: crate::volume_pan::VolumePanNode
    pub volume: Volume,

    /// Whether or not the current sample should start/restart playing (true), or be
    /// paused/stopped (false).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub play: Notify<bool>,

    /// Defines where the sampler should start playing from when
    /// [`SamplerNode::play`] is set to `true`.
    pub play_from: PlayFrom,

    /// How many times a sample should be repeated.
    pub repeat_mode: RepeatMode,

    /// The speed at which to play the sample at. `1.0` means to play the sound at
    /// its original speed, `< 1.0` means to play the sound slower (which will make
    /// it lower-pitched), and `> 1.0` means to play the sound faster (which will
    /// make it higher-pitched).
    pub speed: f64,

    /// If `true`, then mono samples will be converted to stereo during playback.
    ///
    /// By default this is set to `true`.
    pub mono_to_stereo: bool,
    /// If true, then samples will be crossfaded when the playhead or sample is
    /// changed (if a sample was currently playing when the event was sent).
    ///
    /// By default this is set to `true`.
    pub crossfade_on_seek: bool,
    /// If the resulting gain (in raw amplitude, not decibels) is less
    /// than or equal to this value, then the gain will be clamped to
    /// `0.0` (silence).
    ///
    /// By default this is set to `0.00001` (-100 decibels).
    pub min_gain: f32,
}

impl Default for SamplerNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            play: Default::default(),
            play_from: PlayFrom::default(),
            repeat_mode: RepeatMode::default(),
            speed: 1.0,
            mono_to_stereo: true,
            crossfade_on_seek: true,
            min_gain: DEFAULT_MIN_AMP,
        }
    }
}

impl SamplerNode {
    /// Returns an event to clear the sample resource from a sampler node.
    pub fn clear_sample_event() -> NodeEventType {
        NodeEventType::Custom(OwnedGc::new(Box::<Option<SamplerNodeResource>>::new(None)))
    }

    /// Returns an event to set the sample resource for a sampler node from the
    /// given sample resource.
    pub fn set_sample_event<T: SampleResource + Send + Sync + 'static>(sample: T) -> NodeEventType {
        Self::set_resource_event(SamplerNodeResource::from_sample(sample))
    }

    /// Returns an event to set the sample resource for a sampler node from the
    /// given streamed sample resource.
    pub fn set_streamed_sample_event<T: StreamedSample>(sample: T) -> NodeEventType {
        Self::set_resource_event(SamplerNodeResource::from_streamed(sample))
    }

    /// Returns an event to set the sample resource for a sampler node from the
    /// given type-erased sample resource.
    pub fn set_dyn_sample_event(
        sample: ArcGc<dyn SampleResource + Send + Sync + 'static>,
    ) -> NodeEventType {
        Self::set_resource_event(sample.into())
    }

    /// Returns an event to set the sample resource for a sampler node from the
    /// given type-erased streamed sample resource.
    pub fn set_dyn_streamed_sample_event(
        sample: OwnedGcUnsized<dyn StreamedSample>,
    ) -> NodeEventType {
        Self::set_resource_event(sample.into())
    }

    /// Returns an event to set the sample resource for a sampler node.
    pub fn set_resource_event(sample: SamplerNodeResource) -> NodeEventType {
        NodeEventType::Custom(OwnedGc::new(Box::new(Some(sample))))
    }

    /// Returns an event type to sync the `volume` parameter.
    pub fn sync_volume_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::Volume(self.volume),
            path: ParamPath::Single(0),
        }
    }

    /// Returns an event type to sync the `play` parameter.
    pub fn sync_play_event(&self) -> NodeEventType {
        // Diff for Notify<bool> is defined here:
        // https://github.com/BillyDM/Firewheel/blob/380806ce61b3a417eb676a4fd8640da49905ec23/crates/firewheel-core/src/diff/leaf.rs#L247
        let mut bytes: [u8; 20] = [0; 20];
        bytes[0..core::mem::size_of::<u64>()].copy_from_slice(&self.play.id().0.to_ne_bytes());
        bytes[core::mem::size_of::<u64>()] = if *self.play { 1 } else { 0 };

        NodeEventType::Param {
            // TODO: This is not how `Patch` for `Notify<bool>` is implemented.
            data: ParamData::CustomBytes(bytes),
            path: ParamPath::Single(1),
        }
    }

    /// Returns the current playback ID.
    pub fn playback_id(&self) -> PlaybackID {
        self.play.id()
    }

    /// Returns an event type to sync the `play_from` parameter.
    pub fn sync_play_from_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: self.play_from.as_param_data(),
            path: ParamPath::Single(2),
        }
    }

    /// Returns an event type to sync the `playhead` parameter.
    pub fn sync_repeat_mode_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::any(self.repeat_mode),
            path: ParamPath::Single(3),
        }
    }

    /// Returns an event type to sync the `speed` parameter.
    pub fn sync_speed_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::F64(self.speed),
            path: ParamPath::Single(4),
        }
    }

    /// Returns an event type to sync the `mono_to_stereo` parameter.
    pub fn sync_mono_to_stereo_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::Bool(self.mono_to_stereo),
            path: ParamPath::Single(5),
        }
    }

    /// Returns an event type to sync the `crossfade_on_seek` parameter.
    pub fn sync_crossfade_on_seek_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::Bool(self.crossfade_on_seek),
            path: ParamPath::Single(6),
        }
    }

    /// Returns an event type to sync the `min_gain` parameter.
    pub fn sync_min_gain_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::F32(self.min_gain),
            path: ParamPath::Single(7),
        }
    }

    /// Start/restart the sample in this node.
    ///
    /// If a sample is already playing, then it will restart from the beginning.
    pub fn start_or_restart(&mut self) {
        self.play_from = PlayFrom::BEGINNING;
        *self.play = true;
    }

    /// Play the sample in this node from the given playhead.
    pub fn start_from(&mut self, from: PlayFrom) {
        self.play_from = from;
        *self.play = true;
    }

    /// Pause sample playback.
    pub fn pause(&mut self) {
        self.play_from = PlayFrom::Resume;
        *self.play = false;
    }

    /// Resume sample playback.
    pub fn resume(&mut self) {
        *self.play = true;
    }

    /// Stop sample playback.
    ///
    /// Calling [`SamplerNode::resume`] after this will restart the sample from
    /// the beginning.
    pub fn stop(&mut self) {
        self.play_from = PlayFrom::BEGINNING;
        *self.play = false;
    }

    /// Returns `true` if the current state is set to restart the sample.
    pub fn start_or_restart_requested(&self) -> bool {
        *self.play && self.play_from == PlayFrom::BEGINNING
    }

    /// Returns `true` if the current state is set to resume the sample.
    pub fn resume_requested(&self) -> bool {
        *self.play && self.play_from == PlayFrom::Resume
    }

    /// Returns `true` if the current state is set to pause the sample.
    pub fn pause_requested(&self) -> bool {
        !*self.play && self.play_from == PlayFrom::Resume
    }

    /// Returns `true` if the current state is set to stop the sample.
    pub fn stop_requested(&self) -> bool {
        !*self.play && self.play_from != PlayFrom::Resume
    }
}

#[derive(Clone)]
pub struct SamplerState {
    channel: Arc<Mutex<SharedChannel>>,
}

impl SamplerState {
    fn new() -> Self {
        Self {
            channel: Arc::new(Mutex::new(SharedChannel::new())),
        }
    }

    /// Get the current state of this sampler node's processor at this instant
    /// in time.
    pub fn current_processor_state(&self) -> CurrentProcessorState {
        *self.channel.lock().unwrap().proc_state_output.read()
    }

    /// Get the current position of the playhead in units of frames (samples of
    /// a single channel of audio).
    pub fn playhead_frames(&self) -> DurationSamples {
        DurationSamples(
            self.channel
                .lock()
                .unwrap()
                .proc_state_output
                .read()
                .playhead_frames as i64,
        )
    }

    /// Get the current position of the sample playhead in seconds.
    ///
    /// * `sample_rate` - The sample rate of the current audio stream.
    pub fn playhead_seconds(&self, sample_rate: NonZeroU32) -> DurationSeconds {
        DurationSeconds(self.playhead_frames().0 as f64 / sample_rate.get() as f64)
    }

    /// Get the current playback state of the processor at this instant in time.
    pub fn playback_state(&self) -> PlaybackState {
        self.channel
            .lock()
            .unwrap()
            .proc_state_output
            .read()
            .playback_state
    }

    /// Returns `true` if the processor is currently playing a sample at this instant
    /// in time.
    pub fn currently_playing(&self) -> bool {
        self.playback_state() == PlaybackState::Playing
    }

    /// Returns `true` if the processor is currently paused at this instant in time.
    pub fn currently_paused(&self) -> bool {
        self.playback_state() == PlaybackState::Paused
    }

    /// Returns `true` if the the processor has either not started playing a sample yet
    /// or it has finished playing its sample at this instant in time.
    pub fn currently_stopped(&self) -> bool {
        self.playback_state() == PlaybackState::Stopped
    }

    /// Get the current playback state of the processor along with the current
    /// playback ID (the ID of the `play` parameter that the processor currently
    /// has) at this instant in time.
    pub fn playback_state_and_id(&self) -> (PlaybackState, PlaybackID) {
        let mut channel = self.channel.lock().unwrap();
        let s = channel.proc_state_output.read();
        (s.playback_state, s.playback_id)
    }

    /// Get the current position of the playhead in units of frames (samples of
    /// a single channel of audio), corrected with the delay between when the audio clock
    /// was last updated and now.
    ///
    /// Call `FirewheelCtx::audio_clock_instant()` right before calling this method to get
    /// the latest update instant.
    pub fn playhead_frames_corrected(
        &self,
        update_instant: Option<Instant>,
        sample_rate: NonZeroU32,
    ) -> DurationSamples {
        let (playhead_frames, playback_state) = {
            let mut channel = self.channel.lock().unwrap();
            let s = channel.proc_state_output.read();
            (s.playhead_frames, s.playback_state)
        };

        let Some(update_instant) = update_instant else {
            return DurationSamples(playhead_frames as i64);
        };

        if playback_state == PlaybackState::Playing {
            DurationSamples(
                playhead_frames as i64
                    + InstantSeconds(update_instant.elapsed().as_secs_f64())
                        .to_samples(sample_rate)
                        .0,
            )
        } else {
            DurationSamples(playhead_frames as i64)
        }
    }

    /// Get the current position of the playhead in units of seconds, corrected with the
    /// delay between when the audio clock was last updated and now.
    ///
    /// Call `FirewheelCtx::audio_clock_instant()` right before calling this method to get
    /// the latest update instant.
    pub fn playhead_seconds_corrected(
        &self,
        update_instant: Option<Instant>,
        sample_rate: NonZeroU32,
    ) -> DurationSeconds {
        DurationSeconds(
            self.playhead_frames_corrected(update_instant, sample_rate)
                .0 as f64
                / sample_rate.get() as f64,
        )
    }

    /// Returns the last playback ID (the ID of the [`SamplerNode::play`] parameter) that has
    /// finished/stopped.
    pub fn last_finished_playback_id(&self) -> PlaybackID {
        self.channel
            .lock()
            .unwrap()
            .proc_state_output
            .read()
            .last_finished_playback_id
    }

    /// Returns `true` if the given playback with the ID (the ID of the [`SamplerNode::play`]
    /// parameter) has finished/stopped.
    pub fn playback_id_has_finished(&self, id: PlaybackID) -> bool {
        id <= self.last_finished_playback_id()
    }

    /// A score of how suitable this node is to start new work (Play a new sample). The
    /// higher the score, the better the candidate.
    pub fn worker_score(&self, current_worker_params: &SamplerNode) -> u64 {
        let state = self.current_processor_state();

        if current_worker_params.playback_id() <= state.last_finished_playback_id {
            // Sequence has finished playing.
            return u64::MAX;
        }

        if *current_worker_params.play {
            if current_worker_params.playback_id() == state.playback_id
                && state.playback_state == PlaybackState::Stopped
            {
                // Sequence has not started playing yet
                u64::MAX - 4
            } else {
                // The older the sample is, the better it is as a candidate to steal
                // work from.
                state.age_frames
            }
        } else if !state.has_sample_resource {
            u64::MAX
        } else {
            match state.playback_state {
                PlaybackState::Stopped => u64::MAX - 1,
                PlaybackState::Paused => u64::MAX - 2,
                PlaybackState::Playing => u64::MAX - 3,
            }
        }
    }
}

struct SharedChannel {
    proc_state_output: Output<CurrentProcessorState>,
    proc_state_input: Option<Input<CurrentProcessorState>>,
}

impl SharedChannel {
    fn new() -> Self {
        let (proc_state_input, proc_state_output) = triple_buffer::triple_buffer::<
            CurrentProcessorState,
        >(&CurrentProcessorState::default());

        Self {
            proc_state_input: Some(proc_state_input),
            proc_state_output,
        }
    }
}

/// The current state of a [`SamplerNode`]'s processor.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CurrentProcessorState {
    /// Whether or not the processor currently has a sample resource.
    pub has_sample_resource: bool,
    /// The current playback state.
    pub playback_state: PlaybackState,
    /// The current playback ID.
    pub playback_id: PlaybackID,
    /// The ID of the last playback that has finished/stopped.
    pub last_finished_playback_id: PlaybackID,
    /// The current position of the playhead in frames (samples in a single
    /// channel of audio).
    pub playhead_frames: u64,
    /// The age of the current playback in frames (samples in a single channel
    /// of audio).
    pub age_frames: u64,
}

/// The current playback state of a [`SamplerNode`]'s processor.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlaybackState {
    #[default]
    /// The processor has either not started playing a sample yet or it has finished
    /// playing its sample.
    Stopped,
    /// The processor is currently paused.
    Paused,
    /// The processor is currently playing a sample.
    Playing,
}

/// Defines where the sampler should start playing from when
/// [`SamplerNode::play`] is set to `true`.
#[derive(Debug, Clone, Copy, PartialEq, RealtimeClone)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PlayFrom {
    /// When [`SamplerNode::play`] is set to `true`, the sampler will resume
    /// playing from where it last left off.
    Resume,
    /// When [`SamplerNode::play`] is set to `true`, the sampler will begin
    /// playing  from this position in the sample in units of seconds.
    Seconds(f64),
    /// When [`SamplerNode::play`] is set to `true`, the sampler will begin
    /// playing from this position in the sample in units of frames (samples
    /// in a single channel of audio).
    Frames(u64),
}

impl PlayFrom {
    pub const BEGINNING: Self = Self::Frames(0);

    pub fn as_frames(&self, sample_rate: NonZeroU32) -> Option<u64> {
        match *self {
            Self::Resume => None,
            Self::Seconds(seconds) => Some(if seconds <= 0.0 {
                0
            } else {
                (seconds.floor() as u64 * sample_rate.get() as u64)
                    + (seconds.fract() * sample_rate.get() as f64).round() as u64
            }),
            Self::Frames(frames) => Some(frames),
        }
    }

    pub fn as_param_data(&self) -> ParamData {
        match self {
            Self::Resume => ParamData::None,
            Self::Seconds(s) => ParamData::F64(*s),
            Self::Frames(f) => ParamData::U64(*f),
        }
    }
}

impl Default for PlayFrom {
    fn default() -> Self {
        Self::BEGINNING
    }
}

impl Diff for PlayFrom {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            match self {
                Self::Resume => event_queue.push_param(ParamData::None, path),
                Self::Seconds(seconds) => event_queue.push_param(*seconds, path),
                Self::Frames(frames) => event_queue.push_param(*frames, path),
            }
        }
    }
}

impl Patch for PlayFrom {
    type Patch = Self;

    fn patch(data: &ParamData, _path: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::None => Ok(PlayFrom::Resume),
            ParamData::F64(s) => Ok(PlayFrom::Seconds(*s)),
            ParamData::U64(f) => Ok(PlayFrom::Frames(*f)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

/// How many times a sample should be repeated.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Diff, Patch)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RepeatMode {
    /// Play the sample once and then stop.
    #[default]
    PlayOnce,
    /// Repeat the sample the given number of times.
    RepeatMultiple { num_times_to_repeat: u32 },
    /// Repeat the sample endlessly.
    RepeatEndlessly,
}

impl RepeatMode {
    pub fn do_loop(&self, num_times_looped_back: u64) -> bool {
        match self {
            Self::PlayOnce => false,
            &Self::RepeatMultiple {
                num_times_to_repeat,
            } => num_times_looped_back < num_times_to_repeat as u64,
            Self::RepeatEndlessly => true,
        }
    }
}

impl AudioNode for SamplerNode {
    type Configuration = SamplerConfig;

    fn info(&self, config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("sampler")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: config.channels.get(),
            })
            .custom_state(SamplerState::new()))
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        mut cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let stop_declicker_buffers = if config.num_declickers == 0 {
            None
        } else {
            Some(InstanceBuffer::<f32>::new(
                config.num_declickers as usize,
                NonZeroUsize::new(config.channels.get().get() as usize).unwrap(),
                cx.stream_info.declick_frames.get() as usize,
            ))
        };

        let max_block_frames = cx.stream_info.max_block_frames.get() as usize;

        let playing = *self.play;
        let paused = !*self.play && self.play_from == PlayFrom::Resume;
        let playback_state = if playing {
            PlaybackState::Playing
        } else if paused {
            PlaybackState::Paused
        } else {
            PlaybackState::Stopped
        };

        let proc_state = CurrentProcessorState {
            playback_id: self.play.id(),
            playback_state,
            playhead_frames: self
                .play_from
                .as_frames(cx.stream_info.sample_rate)
                .unwrap_or_default(),
            ..Default::default()
        };
        let mut channel = cx
            .custom_state_mut::<SamplerState>()
            .unwrap()
            .channel
            .lock()
            .unwrap();
        let mut shared_proc_state = if let Some(proc_state_input) = channel.proc_state_input.take()
        {
            proc_state_input
        } else {
            *channel = SharedChannel::new();
            channel.proc_state_input.take().unwrap()
        };
        shared_proc_state.write(proc_state);

        Ok(SamplerProcessor {
            config: *config,
            params: *self,
            proc_state,
            shared_proc_state,
            loaded_sample_state: None,
            declicker: Declicker::SettledAt1,
            stop_declicker_buffers,
            stop_declickers: smallvec::smallvec![StopDeclickerState::default(); config.num_declickers as usize],
            num_active_stop_declickers: 0,
            resampler: Some(Resampler::new(config.speed_quality)),
            speed: self.speed.max(MIN_PLAYBACK_SPEED),
            playing,
            paused,
            #[cfg(feature = "scheduled_events")]
            queued_playback_instant: None,
            min_gain: self.min_gain.max(0.0),
            max_block_frames,
            num_out_channels: config.channels.get().get() as usize,
            is_first_process: true,
        })
    }
}

struct SamplerProcessor {
    config: SamplerConfig,
    params: SamplerNode,
    proc_state: CurrentProcessorState,
    shared_proc_state: Input<CurrentProcessorState>,

    loaded_sample_state: Option<LoadedSampleState>,

    declicker: Declicker,

    playing: bool,
    paused: bool,

    stop_declicker_buffers: Option<InstanceBuffer<f32>>,
    stop_declickers: SmallVec<[StopDeclickerState; DEFAULT_NUM_DECLICKERS]>,
    num_active_stop_declickers: usize,

    resampler: Option<Resampler>,
    speed: f64,

    #[cfg(feature = "scheduled_events")]
    queued_playback_instant: Option<EventInstant>,

    min_gain: f32,

    max_block_frames: usize,
    num_out_channels: usize,
    is_first_process: bool,
}

impl SamplerProcessor {
    fn sync_proc_state(&mut self) {
        self.shared_proc_state.write(self.proc_state);
    }

    /// Returns `true` if the sample has finished playing, and also
    /// returns the number of channels that were filled.
    fn process_internal(
        &mut self,
        buffers: &mut [&mut [f32]],
        frames: usize,
        looping: bool,
        extra: &mut ProcExtra,
    ) -> (bool, usize) {
        let (finished_playing, mut channels_filled) = if self.speed != 1.0 {
            // Get around borrow checker.
            let mut resampler = self.resampler.take().unwrap();

            let (finished_playing, channels_filled) =
                resampler.resample_linear(buffers, 0..frames, extra, self, looping);

            self.resampler = Some(resampler);

            (finished_playing, channels_filled)
        } else {
            self.resampler.as_mut().unwrap().reset();

            self.copy_from_sample(buffers, 0..frames, looping)
        };

        let Some(state) = self.loaded_sample_state.as_ref() else {
            return (true, 0);
        };

        if !self.declicker.has_settled() {
            self.declicker.process(
                buffers,
                0..frames,
                &extra.declick_values,
                state.gain,
                DeclickFadeCurve::EqualPower3dB,
            );
        } else if state.gain != 1.0 {
            for b in buffers[..channels_filled].iter_mut() {
                for s in b[..frames].iter_mut() {
                    *s *= state.gain;
                }
            }
        }

        if state.sample_mono_to_stereo {
            let (b0, b1) = buffers.split_first_mut().unwrap();
            b1[0][..frames].copy_from_slice(&b0[..frames]);

            channels_filled = 2;
        }

        (finished_playing, channels_filled)
    }

    /// Fill the buffer with raw data from the sample, starting from the
    /// current playhead. Then increment the playhead.
    ///
    /// Returns `true` if the sample has finished playing, and also
    /// returns the number of channels that were filled.
    fn copy_from_sample(
        &mut self,
        buffers: &mut [&mut [f32]],
        range_in_buffer: Range<usize>,
        looping: bool,
    ) -> (bool, usize) {
        let Some(state) = self.loaded_sample_state.as_mut() else {
            return (true, 0);
        };

        assert!(state.playhead_frames <= state.sample_len_frames);

        let block_frames = range_in_buffer.end - range_in_buffer.start;
        let first_copy_frames =
            if state.playhead_frames + block_frames as u64 > state.sample_len_frames {
                (state.sample_len_frames - state.playhead_frames) as usize
            } else {
                block_frames
            };

        if first_copy_frames > 0 {
            match &mut state.sample {
                SamplerNodeResource::InMemory(sample) => {
                    sample.fill_buffers(
                        buffers,
                        range_in_buffer.start..range_in_buffer.start + first_copy_frames,
                        state.playhead_frames,
                    );
                }
                SamplerNodeResource::Streamed(_) => {
                    todo!()
                }
            }

            state.playhead_frames += first_copy_frames as u64;
        }

        if first_copy_frames < block_frames {
            if looping {
                let mut frames_copied = first_copy_frames;

                while frames_copied < block_frames {
                    let copy_frames = ((block_frames - frames_copied) as u64)
                        .min(state.sample_len_frames)
                        as usize;

                    match &mut state.sample {
                        SamplerNodeResource::InMemory(sample) => {
                            sample.fill_buffers(
                                buffers,
                                range_in_buffer.start + frames_copied
                                    ..range_in_buffer.start + frames_copied + copy_frames,
                                0,
                            );
                        }
                        SamplerNodeResource::Streamed(_) => {
                            todo!()
                        }
                    }

                    state.playhead_frames = copy_frames as u64;
                    state.num_times_looped_back += 1;

                    frames_copied += copy_frames;
                }
            } else {
                let n_channels = buffers.len().min(state.sample_num_channels.get());
                for b in buffers[..n_channels].iter_mut() {
                    b[range_in_buffer.start + first_copy_frames..range_in_buffer.end].fill(0.0);
                }

                return (true, n_channels);
            }
        }

        (false, buffers.len().min(state.sample_num_channels.get()))
    }

    fn currently_processing_sample(&self) -> bool {
        if self.loaded_sample_state.is_none() {
            false
        } else {
            self.playing || (self.paused && !self.declicker.has_settled())
        }
    }

    fn num_channels_filled(&self) -> usize {
        if let Some(state) = &self.loaded_sample_state {
            if state.sample_mono_to_stereo {
                2
            } else {
                state.sample_num_channels.get().min(self.num_out_channels)
            }
        } else {
            0
        }
    }

    fn stop(&mut self, extra: &mut ProcExtra) {
        if self.currently_processing_sample() {
            // Fade out the sample into a temporary look-ahead
            // buffer to declick.

            self.declicker.fade_to_0(&extra.declick_values);

            // Work around the borrow checker.
            if let Some(mut stop_declicker_buffers) = self.stop_declicker_buffers.take() {
                if self.num_active_stop_declickers < stop_declicker_buffers.num_instances() {
                    let declicker_i = self
                        .stop_declickers
                        .iter()
                        .enumerate()
                        .find_map(|(i, d)| if d.frames_left == 0 { Some(i) } else { None })
                        .unwrap();

                    let n_channels = self.num_channels_filled();

                    let fade_out_frames = stop_declicker_buffers.frames();

                    self.stop_declickers[declicker_i].frames_left = fade_out_frames;
                    self.stop_declickers[declicker_i].channels = n_channels;

                    let mut tmp_buffers = stop_declicker_buffers
                        .instance_mut::<MAX_OUT_CHANNELS>(declicker_i, n_channels, fade_out_frames)
                        .unwrap();

                    self.process_internal(&mut tmp_buffers, fade_out_frames, false, extra);

                    self.num_active_stop_declickers += 1;
                }

                self.stop_declicker_buffers = Some(stop_declicker_buffers);
            }
        }

        if let Some(state) = &mut self.loaded_sample_state {
            state.playhead_frames = 0;
            state.num_times_looped_back = 0;
        }

        self.declicker.reset_to_1();

        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }

    fn load_sample(&mut self, sample: SamplerNodeResource) {
        let mut gain = self.params.volume.amp_clamped(self.min_gain);
        if gain > 0.99999 && gain < 1.00001 {
            gain = 1.0;
        }

        let (sample_len_frames, sample_num_channels) = match &sample {
            SamplerNodeResource::InMemory(s) => (s.len_frames(), s.num_channels()),
            SamplerNodeResource::Streamed(s) => (s.len_frames(), s.num_channels()),
        };

        let sample_mono_to_stereo = self.params.mono_to_stereo
            && self.num_out_channels > 1
            && sample_num_channels.get() == 1;

        self.loaded_sample_state = Some(LoadedSampleState {
            sample,
            sample_len_frames,
            sample_num_channels,
            sample_mono_to_stereo,
            gain,
            playhead_frames: 0,
            num_times_looped_back: 0,
        });
    }
}

impl AudioNodeProcessor for SamplerProcessor {
    fn events(&mut self, info: &ProcInfo, events: &mut ProcEvents, extra: &mut ProcExtra) {
        let is_first_process = self.is_first_process;
        self.is_first_process = false;

        let mut new_playing: Option<bool> = if is_first_process {
            Some(self.playing)
        } else {
            None
        };
        let mut new_sample = None;
        let mut repeat_mode_changed = false;
        let mut speed_changed = false;
        let mut volume_changed = false;
        let mut proc_state_changed = false;

        #[cfg(feature = "scheduled_events")]
        let mut playback_instant: Option<EventInstant> = None;

        #[cfg(feature = "scheduled_events")]
        for (mut event, timestamp) in events.drain_with_timestamps() {
            let mut s = None;
            if event.downcast_swap::<Option<SamplerNodeResource>>(&mut s) {
                new_sample = Some(s);
            }

            if let Some(patch) = SamplerNode::patch_event(&event) {
                match patch {
                    SamplerNodePatch::Volume(_) => volume_changed = true,
                    SamplerNodePatch::Play(play) => {
                        playback_instant = timestamp;
                        new_playing = Some(*play);
                    }
                    SamplerNodePatch::RepeatMode(_) => repeat_mode_changed = true,
                    SamplerNodePatch::Speed(_) => speed_changed = true,
                    SamplerNodePatch::MinGain(min_gain) => {
                        self.min_gain = min_gain.max(0.0);
                    }
                    _ => {}
                }

                self.params.apply(patch);
            }
        }

        #[cfg(not(feature = "scheduled_events"))]
        for mut event in events.drain() {
            let mut s = None;
            if event.downcast_swap::<Option<SamplerNodeResource>>(&mut s) {
                new_sample = Some(s);
            }

            if let Some(patch) = SamplerNode::patch_event(&event) {
                match patch {
                    SamplerNodePatch::Volume(_) => volume_changed = true,
                    SamplerNodePatch::Play(play) => {
                        new_playing = Some(*play);
                    }
                    SamplerNodePatch::RepeatMode(_) => repeat_mode_changed = true,
                    SamplerNodePatch::Speed(_) => speed_changed = true,
                    SamplerNodePatch::MinGain(min_gain) => {
                        self.min_gain = min_gain.max(0.0);
                    }
                    _ => {}
                }

                self.params.apply(patch);
            }
        }

        if speed_changed {
            self.speed = self.params.speed.max(MIN_PLAYBACK_SPEED);

            if self.speed > 0.99999 && self.speed < 1.00001 {
                self.speed = 1.0;
            }
        }

        if volume_changed && let Some(loaded_sample) = &mut self.loaded_sample_state {
            loaded_sample.gain = self.params.volume.amp_clamped(self.min_gain);
            if loaded_sample.gain > 0.99999 && loaded_sample.gain < 1.00001 {
                loaded_sample.gain = 1.0;
            }
        }

        if repeat_mode_changed && let Some(loaded_sample) = &mut self.loaded_sample_state {
            loaded_sample.num_times_looped_back = 0;
        }

        if let Some(maybe_sample) = new_sample {
            self.proc_state.has_sample_resource = maybe_sample.is_some();
            proc_state_changed = true;

            self.stop(extra);

            #[cfg(feature = "scheduled_events")]
            if new_playing == Some(true)
                && playback_instant.is_none()
                && let Some(queued_playback_instant) = self.queued_playback_instant.take()
                && queued_playback_instant.to_samples(info).is_some()
            {
                playback_instant = Some(queued_playback_instant);
            }

            self.loaded_sample_state = None;

            if let Some(sample) = maybe_sample {
                self.load_sample(sample);
            }
        }

        if let Some(mut new_playing) = new_playing {
            self.paused = false;
            self.proc_state.last_finished_playback_id = self.proc_state.playback_id;
            self.proc_state.playback_id = self.params.play.id();
            self.proc_state.age_frames = 0;
            proc_state_changed = true;

            if new_playing {
                let mut playhead_frames_at_play_instant = None;

                if self.params.play_from == PlayFrom::Resume {
                    // Resume
                    if self.playing && !is_first_process {
                        // Sample is already playing, no need to do anything.
                        #[cfg(feature = "scheduled_events")]
                        {
                            self.queued_playback_instant = None;
                        }
                    } else if let Some(loaded_sample_state) = &self.loaded_sample_state {
                        playhead_frames_at_play_instant = Some(loaded_sample_state.playhead_frames);
                    }
                } else {
                    // Play from the given playhead
                    if let Some(loaded_sample_state) = &mut self.loaded_sample_state {
                        loaded_sample_state.num_times_looped_back = 0;
                        playhead_frames_at_play_instant =
                            Some(self.params.play_from.as_frames(info.sample_rate).unwrap());
                    } else {
                        #[cfg(feature = "scheduled_events")]
                        {
                            self.queued_playback_instant = playback_instant;
                        }
                    }
                }

                if let Some(playhead_frames_at_play_instant) = playhead_frames_at_play_instant {
                    let loaded_sample_state = self.loaded_sample_state.as_mut().unwrap();
                    let prev_playhead_frames = loaded_sample_state.playhead_frames;

                    #[cfg(feature = "scheduled_events")]
                    let mut new_playhead_frames = if let Some(playback_instant) = playback_instant {
                        let playback_instant_samples = playback_instant
                            .to_samples(info)
                            .unwrap_or(info.clock_samples);
                        let delay = if playback_instant_samples < info.clock_samples {
                            (info.clock_samples - playback_instant_samples).0 as u64
                        } else {
                            0
                        };

                        playhead_frames_at_play_instant + delay
                    } else {
                        playhead_frames_at_play_instant
                    };

                    #[cfg(not(feature = "scheduled_events"))]
                    let mut new_playhead_frames = playhead_frames_at_play_instant;

                    if new_playhead_frames >= loaded_sample_state.sample_len_frames {
                        match self.params.repeat_mode {
                            RepeatMode::PlayOnce => {
                                new_playhead_frames = loaded_sample_state.sample_len_frames
                            }
                            RepeatMode::RepeatEndlessly => {
                                while new_playhead_frames >= loaded_sample_state.sample_len_frames {
                                    new_playhead_frames -= loaded_sample_state.sample_len_frames;
                                    loaded_sample_state.num_times_looped_back += 1;
                                }
                            }
                            RepeatMode::RepeatMultiple {
                                num_times_to_repeat,
                            } => {
                                while new_playhead_frames >= loaded_sample_state.sample_len_frames {
                                    if loaded_sample_state.num_times_looped_back
                                        == num_times_to_repeat as u64
                                    {
                                        new_playhead_frames = loaded_sample_state.sample_len_frames;
                                        break;
                                    }

                                    new_playhead_frames -= loaded_sample_state.sample_len_frames;
                                    loaded_sample_state.num_times_looped_back += 1;
                                }
                            }
                        }
                    }

                    if prev_playhead_frames != new_playhead_frames {
                        self.stop(extra);

                        self.loaded_sample_state.as_mut().unwrap().playhead_frames =
                            new_playhead_frames;

                        self.proc_state.playhead_frames = new_playhead_frames;
                    }

                    if new_playhead_frames
                        == self.loaded_sample_state.as_ref().unwrap().sample_len_frames
                    {
                        self.proc_state.playhead_frames = new_playhead_frames;

                        new_playing = false;
                        self.proc_state.last_finished_playback_id = self.params.play.id();
                    } else if new_playhead_frames != 0
                        || (self.num_active_stop_declickers > 0 && self.params.crossfade_on_seek)
                    {
                        self.declicker.reset_to_0();
                        self.declicker.fade_to_1(&extra.declick_values);
                    } else {
                        self.declicker.reset_to_1();
                    }

                    #[cfg(feature = "scheduled_events")]
                    {
                        self.queued_playback_instant = None;
                    }
                }
            } else if self.params.play_from == PlayFrom::Resume {
                // Pause
                self.declicker.fade_to_0(&extra.declick_values);
                self.paused = true;
            } else {
                // Stop
                self.stop(extra);
                self.proc_state.last_finished_playback_id = self.params.play.id();
            }

            self.playing = new_playing;

            self.proc_state.playback_state = if self.playing {
                PlaybackState::Playing
            } else if self.paused {
                PlaybackState::Paused
            } else {
                PlaybackState::Stopped
            };
        }

        if proc_state_changed {
            self.sync_proc_state();
        }
    }

    fn bypassed(&mut self, _bypassed: bool) {
        self.declicker.reset_to_target();
        self.num_active_stop_declickers = 0;
    }

    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let currently_processing_sample = self.currently_processing_sample();

        if !currently_processing_sample && self.num_active_stop_declickers == 0 {
            return ProcessStatus::ClearAllOutputs;
        }

        let mut num_filled_channels = 0;

        if currently_processing_sample {
            let sample_state = self.loaded_sample_state.as_ref().unwrap();

            let looping = self
                .params
                .repeat_mode
                .do_loop(sample_state.num_times_looped_back);

            let (finished, n_channels) =
                self.process_internal(buffers.outputs, info.frames, looping, extra);

            num_filled_channels = n_channels;

            self.proc_state.playhead_frames =
                self.loaded_sample_state.as_ref().unwrap().playhead_frames;

            if finished {
                self.playing = false;

                self.proc_state.playback_state = PlaybackState::Stopped;
                self.proc_state.last_finished_playback_id = self.params.play.id();
            } else {
                self.proc_state.age_frames = self
                    .proc_state
                    .age_frames
                    .saturating_add(info.frames as u64);
            }

            self.sync_proc_state();
        }

        for (i, out_buf) in buffers
            .outputs
            .iter_mut()
            .enumerate()
            .skip(num_filled_channels)
        {
            if !info.out_silence_mask.is_channel_silent(i) {
                out_buf[..info.frames].fill(0.0);
            }
        }

        if self.num_active_stop_declickers > 0 {
            let tmp_buffers = self.stop_declicker_buffers.as_ref().unwrap();
            let fade_out_frames = tmp_buffers.frames();

            for (declicker_i, declicker) in self.stop_declickers.iter_mut().enumerate() {
                if declicker.frames_left == 0 {
                    continue;
                }

                let tmp_buffers = tmp_buffers
                    .instance::<MAX_OUT_CHANNELS>(declicker_i, declicker.channels, fade_out_frames)
                    .unwrap();

                let copy_frames = info.frames.min(declicker.frames_left);
                let start_frame = fade_out_frames - declicker.frames_left;

                for (out_buf, tmp_buf) in buffers.outputs.iter_mut().zip(tmp_buffers.iter()) {
                    for (os, &ts) in out_buf[..copy_frames]
                        .iter_mut()
                        .zip(tmp_buf[start_frame..start_frame + copy_frames].iter())
                    {
                        *os += ts;
                    }
                }

                declicker.frames_left -= copy_frames;
                if declicker.frames_left == 0 {
                    self.num_active_stop_declickers -= 1;
                }

                num_filled_channels = num_filled_channels.max(declicker.channels);
            }
        }

        let out_silence_mask = if num_filled_channels >= self.num_out_channels {
            SilenceMask::NONE_SILENT
        } else {
            let mut mask = SilenceMask::new_all_silent(self.num_out_channels);
            for i in 0..num_filled_channels {
                mask.set_channel(i, false);
            }
            mask
        };

        ProcessStatus::OutputsModifiedWithMask(MaskType::Silence(out_silence_mask))
    }

    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        if stream_info.sample_rate != stream_info.prev_sample_rate {
            self.stop_declicker_buffers = if self.config.num_declickers == 0 {
                None
            } else {
                Some(InstanceBuffer::<f32>::new(
                    self.config.num_declickers as usize,
                    NonZeroUsize::new(self.config.channels.get().get() as usize).unwrap(),
                    stream_info.declick_frames.get() as usize,
                ))
            };

            // The sample rate has changed, meaning that the sample resources now have
            // the incorrect sample rate and the user must reload them.
            self.loaded_sample_state = None;
            self.playing = false;
            self.paused = false;
            self.proc_state.playback_state = PlaybackState::Stopped;
            self.proc_state.last_finished_playback_id = self.params.playback_id();
            self.sync_proc_state();
        }
    }
}

struct LoadedSampleState {
    sample: SamplerNodeResource,
    sample_len_frames: u64,
    sample_num_channels: NonZeroUsize,
    sample_mono_to_stereo: bool,
    gain: f32,
    playhead_frames: u64,
    num_times_looped_back: u64,
}

#[derive(Default, Clone, Copy)]
struct StopDeclickerState {
    frames_left: usize,
    channels: usize,
}
