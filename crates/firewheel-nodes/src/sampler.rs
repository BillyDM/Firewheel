// TODO: The logic in this has become increadibly complex and error-prone. I plan
// on rewriting the sampler engine using a state machine.

use firewheel_core::clock::{DurationSamples, DurationSeconds};
use firewheel_core::node::{ProcBuffers, ProcExtra, ProcStreamCtx};
#[cfg(not(feature = "std"))]
use num_traits::Float;

use bevy_platform::sync::atomic::{AtomicU64, Ordering};
use bevy_platform::time::Instant;
use core::sync::atomic::AtomicU32;
use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};
use firewheel_core::diff::{EventQueue, PatchError, PathBuilder, RealtimeClone};
use smallvec::SmallVec;

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    clock::InstantSeconds,
    collector::ArcGc,
    diff::{Diff, Notify, ParamPath, Patch},
    dsp::{
        buffer::InstanceBuffer,
        declick::{DeclickFadeCurve, Declicker},
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    event::{NodeEventType, ParamData, ProcEvents},
    mask::{MaskType, SilenceMask},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcInfo,
        ProcessStatus,
    },
    sample_resource::SampleResource,
    StreamInfo,
};

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

pub const MAX_OUT_CHANNELS: usize = 8;
pub const DEFAULT_NUM_DECLICKERS: usize = 2;
pub const MIN_PLAYBACK_SPEED: f64 = 0.0000001;

/// The configuration of a [`SamplerNode`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SamplerConfig {
    /// The number of channels in this node.
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
#[derive(Clone, Diff, Patch, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct SamplerNode {
    /// The sample resource to use.
    pub sample: Option<ArcGc<dyn SampleResource>>,

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
    /// If the resutling gain (in raw amplitude, not decibels) is less
    /// than or equal to this value, then the gain will be clamped to
    /// `0.0` (silence).
    ///
    /// By default this is set to `0.00001` (-100 decibels).
    pub min_gain: f32,
}

impl Default for SamplerNode {
    fn default() -> Self {
        Self {
            sample: None,
            volume: Volume::default(),
            play: Default::default(),
            play_from: PlayFrom::default(),
            repeat_mode: RepeatMode::default(),
            speed: 1.0,
            mono_to_stereo: true,
            crossfade_on_seek: true,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }
}

impl core::fmt::Debug for SamplerNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_struct("SamplerNode");
        f.field("has_sample", &self.sample.is_some());
        f.field("volume", &self.volume);
        f.field("play", &self.play);
        f.field("play_from", &self.play_from);
        f.field("repeat_mode", &self.repeat_mode);
        f.field("speed", &self.speed);
        f.field("mono_to_stereo", &self.mono_to_stereo);
        f.field("crossfade_on_seek", &self.crossfade_on_seek);
        f.field("min_gain", &self.min_gain);
        f.finish()
    }
}

impl SamplerNode {
    /// Set the parameters to a play a single sample.
    pub fn set_sample(&mut self, sample: ArcGc<dyn SampleResource>) {
        self.sample = Some(sample);
    }

    /// Returns an event type to sync the `sample` parameter.
    pub fn sync_sample_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::any(self.sample.clone()),
            path: ParamPath::Single(0),
        }
    }

    /// Returns an event type to sync the `volume` parameter.
    pub fn sync_volume_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::Volume(self.volume),
            path: ParamPath::Single(1),
        }
    }

    /// Returns an event type to sync the `play` parameter.
    pub fn sync_play_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::Bool(*self.play),
            path: ParamPath::Single(2),
        }
    }

    /// Returns an event type to sync the `play_from` parameter.
    pub fn sync_play_from_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: self.play_from.as_param_data(),
            path: ParamPath::Single(3),
        }
    }

    /// Returns an event type to sync the `playhead` parameter.
    pub fn sync_repeat_mode_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::any(self.repeat_mode),
            path: ParamPath::Single(4),
        }
    }

    /// Returns an event type to sync the `speed` parameter.
    pub fn sync_speed_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::F64(self.speed),
            path: ParamPath::Single(5),
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
    shared_state: ArcGc<SharedState>,
}

impl SamplerState {
    fn new() -> Self {
        Self {
            shared_state: ArcGc::new(SharedState::default()),
        }
    }

    /// Get the current position of the playhead in units of frames (samples of
    /// a single channel of audio).
    pub fn playhead_frames(&self) -> DurationSamples {
        DurationSamples(
            self.shared_state
                .sample_playhead_frames
                .load(Ordering::Relaxed) as i64,
        )
    }

    /// Get the current position of the sample playhead in seconds.
    ///
    /// * `sample_rate` - The sample rate of the current audio stream.
    pub fn playhead_seconds(&self, sample_rate: NonZeroU32) -> DurationSeconds {
        DurationSeconds(self.playhead_frames().0 as f64 / sample_rate.get() as f64)
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
        let frames = self.playhead_frames();

        let Some(update_instant) = update_instant else {
            return frames;
        };

        if SharedPlaybackState::from_u32(self.shared_state.playback_state.load(Ordering::Relaxed))
            == SharedPlaybackState::Playing
        {
            DurationSamples(
                frames.0
                    + InstantSeconds(update_instant.elapsed().as_secs_f64())
                        .to_samples(sample_rate)
                        .0 as i64,
            )
        } else {
            frames
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

    /// Returns `true` if the sample is currently playing.
    pub fn playing(&self) -> bool {
        SharedPlaybackState::from_u32(self.shared_state.playback_state.load(Ordering::Relaxed))
            == SharedPlaybackState::Playing
    }

    /// Returns `true` if the sample is currently paused.
    pub fn paused(&self) -> bool {
        SharedPlaybackState::from_u32(self.shared_state.playback_state.load(Ordering::Relaxed))
            == SharedPlaybackState::Paused
    }

    /// Returns `true` if the sample has either not started playing yet or has finished
    /// playing.
    pub fn stopped(&self) -> bool {
        SharedPlaybackState::from_u32(self.shared_state.playback_state.load(Ordering::Relaxed))
            == SharedPlaybackState::Stopped
    }

    /// Manually set the shared `playing` flag. This can be useful to account for the delay
    /// between sending a play event and the node's processor receiving that event.
    pub fn mark_playing(&self) {
        self.shared_state
            .playback_state
            .store(SharedPlaybackState::Playing as u32, Ordering::Relaxed);
    }

    /// Manually set the shared `paused` flag. This can be useful to account for the delay
    /// between sending a play event and the node's processor receiving that event.
    pub fn mark_paused(&self) {
        self.shared_state
            .playback_state
            .store(SharedPlaybackState::Paused as u32, Ordering::Relaxed);
    }

    /// Manually set the shared `stopped` flag. This can be useful to account for the delay
    /// between sending a play event and the node's processor receiving that event.
    pub fn mark_stopped(&self) {
        self.shared_state
            .playback_state
            .store(SharedPlaybackState::Stopped as u32, Ordering::Relaxed);
    }

    /// Returns the ID stored in the "finished" flag.
    pub fn finished(&self) -> u64 {
        self.shared_state.finished.load(Ordering::Relaxed)
    }

    /// Clears the "finished" flag.
    pub fn clear_finished(&self) {
        self.shared_state.finished.store(0, Ordering::Relaxed);
    }

    /// A score of how suitable this node is to start new work (Play a new sample). The
    /// higher the score, the better the candidate.
    pub fn worker_score(&self, params: &SamplerNode) -> u64 {
        if params.sample.is_some() {
            let playback_state = SharedPlaybackState::from_u32(
                self.shared_state.playback_state.load(Ordering::Relaxed),
            );

            if *params.play {
                let playhead_frames = self.playhead_frames();

                if playback_state == SharedPlaybackState::Stopped {
                    if playhead_frames.0 > 0 {
                        // Sequence has likely finished playing.
                        u64::MAX - 4
                    } else {
                        // Sequence has likely not started playing yet.
                        u64::MAX - 5
                    }
                } else {
                    // The older the sample is, the better it is as a candidate to steal
                    // work from.
                    playhead_frames.0 as u64
                }
            } else {
                match playback_state {
                    SharedPlaybackState::Stopped => u64::MAX - 1,
                    SharedPlaybackState::Paused => u64::MAX - 2,
                    SharedPlaybackState::Playing => u64::MAX - 3,
                }
            }
        } else {
            u64::MAX
        }
    }
}

/// Defines where the sampler should start playing from when
/// [`SamplerNode::play`] is set to `true`.
#[derive(Debug, Clone, Copy, PartialEq, RealtimeClone)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
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
            event_queue.push_param(self.as_param_data(), path);
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

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("sampler")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: config.channels.get(),
            })
            .custom_state(SamplerState::new())
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let stop_declicker_buffers = if config.num_declickers == 0 {
            None
        } else {
            Some(InstanceBuffer::<f32, MAX_OUT_CHANNELS>::new(
                config.num_declickers as usize,
                NonZeroUsize::new(config.channels.get().get() as usize).unwrap(),
                cx.stream_info.declick_frames.get() as usize,
            ))
        };

        SamplerProcessor {
            config: config.clone(),
            params: self.clone(),
            shared_state: ArcGc::clone(&cx.custom_state::<SamplerState>().unwrap().shared_state),
            loaded_sample_state: None,
            declicker: Declicker::SettledAt1,
            stop_declicker_buffers,
            stop_declickers: smallvec::smallvec![StopDeclickerState::default(); config.num_declickers as usize],
            num_active_stop_declickers: 0,
            resampler: Some(Resampler::new(config.speed_quality)),
            speed: self.speed.max(MIN_PLAYBACK_SPEED),
            playing: *self.play,
            paused: !*self.play && self.play_from == PlayFrom::Resume,
            #[cfg(feature = "scheduled_events")]
            queued_playback_instant: None,
            min_gain: self.min_gain.max(0.0),
            is_first_process: true,
            max_block_frames: cx.stream_info.max_block_frames.get() as usize,
        }
    }
}

struct SamplerProcessor {
    config: SamplerConfig,
    params: SamplerNode,
    shared_state: ArcGc<SharedState>,

    loaded_sample_state: Option<LoadedSampleState>,

    declicker: Declicker,

    playing: bool,
    paused: bool,

    stop_declicker_buffers: Option<InstanceBuffer<f32, MAX_OUT_CHANNELS>>,
    stop_declickers: SmallVec<[StopDeclickerState; DEFAULT_NUM_DECLICKERS]>,
    num_active_stop_declickers: usize,

    resampler: Option<Resampler>,
    speed: f64,

    #[cfg(feature = "scheduled_events")]
    queued_playback_instant: Option<EventInstant>,

    min_gain: f32,

    is_first_process: bool,
    max_block_frames: usize,
}

impl SamplerProcessor {
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
            state.sample.fill_buffers(
                buffers,
                range_in_buffer.start..range_in_buffer.start + first_copy_frames,
                state.playhead_frames,
            );

            state.playhead_frames += first_copy_frames as u64;
        }

        if first_copy_frames < block_frames {
            if looping {
                let mut frames_copied = first_copy_frames;

                while frames_copied < block_frames {
                    let copy_frames = ((block_frames - frames_copied) as u64)
                        .min(state.sample_len_frames)
                        as usize;

                    state.sample.fill_buffers(
                        buffers,
                        range_in_buffer.start + frames_copied
                            ..range_in_buffer.start + frames_copied + copy_frames,
                        0,
                    );

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
        if self.params.sample.is_none() {
            false
        } else {
            self.playing || (self.paused && !self.declicker.has_settled())
        }
    }

    fn num_channels_filled(&self, num_out_channels: usize) -> usize {
        if let Some(state) = &self.loaded_sample_state {
            if state.sample_mono_to_stereo {
                2
            } else {
                state.sample_num_channels.get().min(num_out_channels)
            }
        } else {
            0
        }
    }

    fn stop(&mut self, num_out_channels: usize, extra: &mut ProcExtra) {
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

                    let n_channels = self.num_channels_filled(num_out_channels);

                    let fade_out_frames = stop_declicker_buffers.frames();

                    self.stop_declickers[declicker_i].frames_left = fade_out_frames;
                    self.stop_declickers[declicker_i].channels = n_channels;

                    let mut tmp_buffers = stop_declicker_buffers
                        .instance_mut(declicker_i, n_channels, fade_out_frames)
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

    fn load_sample(&mut self, sample: ArcGc<dyn SampleResource>, num_out_channels: usize) {
        let mut gain = self.params.volume.amp_clamped(self.min_gain);
        if gain > 0.99999 && gain < 1.00001 {
            gain = 1.0;
        }

        let sample_len_frames = sample.len_frames();
        let sample_num_channels = sample.num_channels();

        let sample_mono_to_stereo =
            self.params.mono_to_stereo && num_out_channels > 1 && sample_num_channels.get() == 1;

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
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let mut sample_changed = self.is_first_process;
        let mut repeat_mode_changed = false;
        let mut speed_changed = false;
        let mut volume_changed = false;
        let mut new_playing: Option<bool> = if self.is_first_process {
            Some(self.playing)
        } else {
            None
        };

        #[cfg(feature = "scheduled_events")]
        let mut playback_instant: Option<EventInstant> = None;

        #[cfg(not(feature = "scheduled_events"))]
        for patch in events.drain_patches::<SamplerNode>() {
            match patch {
                SamplerNodePatch::Sample(_) => sample_changed = true,
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

        #[cfg(feature = "scheduled_events")]
        for (patch, timestamp) in events.drain_patches_with_timestamps::<SamplerNode>() {
            match patch {
                SamplerNodePatch::Sample(_) => sample_changed = true,
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

        if speed_changed {
            self.speed = self.params.speed.max(MIN_PLAYBACK_SPEED);

            if self.speed > 0.99999 && self.speed < 1.00001 {
                self.speed = 1.0;
            }
        }

        if volume_changed {
            if let Some(loaded_sample) = &mut self.loaded_sample_state {
                loaded_sample.gain = self.params.volume.amp_clamped(self.min_gain);
                if loaded_sample.gain > 0.99999 && loaded_sample.gain < 1.00001 {
                    loaded_sample.gain = 1.0;
                }
            }
        }

        if repeat_mode_changed {
            if let Some(loaded_sample) = &mut self.loaded_sample_state {
                loaded_sample.num_times_looped_back = 0;
            }
        }

        if sample_changed {
            self.stop(buffers.outputs.len(), extra);

            #[cfg(feature = "scheduled_events")]
            if new_playing == Some(true) && playback_instant.is_none() {
                if let Some(queued_playback_instant) = self.queued_playback_instant.take() {
                    if queued_playback_instant.to_samples(info).is_some() {
                        playback_instant = Some(queued_playback_instant);
                    }
                }
            }

            self.loaded_sample_state = None;

            if let Some(sample) = &self.params.sample {
                self.load_sample(ArcGc::clone(sample), buffers.outputs.len());
            }
        }

        if let Some(mut new_playing) = new_playing {
            self.paused = false;

            if new_playing {
                let mut playhead_frames_at_play_instant = None;

                if self.params.play_from == PlayFrom::Resume {
                    // Resume
                    if self.playing && !self.is_first_process {
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
                        self.stop(buffers.outputs.len(), extra);

                        self.loaded_sample_state.as_mut().unwrap().playhead_frames =
                            new_playhead_frames;

                        self.shared_state
                            .sample_playhead_frames
                            .store(new_playhead_frames, Ordering::Relaxed);
                    }

                    if new_playhead_frames
                        == self.loaded_sample_state.as_ref().unwrap().sample_len_frames
                    {
                        self.shared_state
                            .finished
                            .store(self.params.play.id(), Ordering::Relaxed);

                        new_playing = false;
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
            } else {
                if self.params.play_from == PlayFrom::Resume {
                    // Pause
                    self.declicker.fade_to_0(&extra.declick_values);
                    self.paused = true;
                } else {
                    // Stop
                    self.stop(buffers.outputs.len(), extra);
                    self.shared_state
                        .finished
                        .store(self.params.play.id(), Ordering::Relaxed);
                }
            }

            self.playing = new_playing;
        }

        self.is_first_process = false;

        self.shared_state.playback_state.store(
            if self.playing {
                SharedPlaybackState::Playing
            } else if self.paused {
                SharedPlaybackState::Paused
            } else {
                SharedPlaybackState::Stopped
            } as u32,
            Ordering::Relaxed,
        );

        let currently_processing_sample = self.currently_processing_sample();

        if !currently_processing_sample && self.num_active_stop_declickers == 0 {
            return ProcessStatus::ClearAllOutputs;
        }

        let mut num_filled_channels = 0;

        if currently_processing_sample && self.params.sample.is_some() {
            let sample_state = self.loaded_sample_state.as_ref().unwrap();

            let looping = self
                .params
                .repeat_mode
                .do_loop(sample_state.num_times_looped_back);

            let (finished, n_channels) =
                self.process_internal(buffers.outputs, info.frames, looping, extra);

            num_filled_channels = n_channels;

            self.shared_state.sample_playhead_frames.store(
                self.loaded_sample_state.as_ref().unwrap().playhead_frames,
                Ordering::Relaxed,
            );

            if finished {
                self.playing = false;

                self.shared_state
                    .playback_state
                    .store(SharedPlaybackState::Stopped as u32, Ordering::Relaxed);
                self.shared_state
                    .finished
                    .store(self.params.play.id(), Ordering::Relaxed);
            }
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
                    .instance(declicker_i, declicker.channels, fade_out_frames)
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

        let out_silence_mask = if num_filled_channels >= buffers.outputs.len() {
            SilenceMask::NONE_SILENT
        } else {
            let mut mask = SilenceMask::new_all_silent(buffers.outputs.len());
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
                Some(InstanceBuffer::<f32, MAX_OUT_CHANNELS>::new(
                    self.config.num_declickers as usize,
                    NonZeroUsize::new(self.config.channels.get().get() as usize).unwrap(),
                    stream_info.declick_frames.get() as usize,
                ))
            };

            // The sample rate has changed, meaning that the sample resources now have
            // the incorrect sample rate and the user must reload them.
            self.params.sample = None;
            self.loaded_sample_state = None;
            self.playing = false;
            self.paused = false;
            self.shared_state
                .playback_state
                .store(SharedPlaybackState::Stopped as u32, Ordering::Relaxed);
            self.shared_state.finished.store(0, Ordering::Relaxed);
        }
    }
}

struct SharedState {
    sample_playhead_frames: AtomicU64,
    playback_state: AtomicU32,
    finished: AtomicU64,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            sample_playhead_frames: AtomicU64::new(0),
            playback_state: AtomicU32::new(SharedPlaybackState::Stopped as u32),
            finished: AtomicU64::new(0),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SharedPlaybackState {
    Stopped = 0,
    Paused,
    Playing,
}

impl SharedPlaybackState {
    fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::Paused,
            2 => Self::Playing,
            _ => Self::Stopped,
        }
    }
}

struct LoadedSampleState {
    sample: ArcGc<dyn SampleResource>,
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

struct Resampler {
    fract_in_frame: f64,
    is_first_process: bool,
    prev_speed: f64,
    _quality: PlaybackSpeedQuality,
    wraparound_buffer: [[f32; 2]; MAX_OUT_CHANNELS],
}

impl Resampler {
    pub fn new(quality: PlaybackSpeedQuality) -> Self {
        Self {
            fract_in_frame: 0.0,
            is_first_process: true,
            prev_speed: 1.0,
            _quality: quality,
            wraparound_buffer: [[0.0; 2]; MAX_OUT_CHANNELS],
        }
    }

    pub fn resample_linear(
        &mut self,
        out_buffers: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        extra: &mut ProcExtra,
        processor: &mut SamplerProcessor,
        looping: bool,
    ) -> (bool, usize) {
        let total_out_frames = out_buffer_range.end - out_buffer_range.start;

        assert_ne!(total_out_frames, 0);

        let in_frame_start = if self.is_first_process {
            self.prev_speed = processor.speed;
            self.fract_in_frame = 0.0;

            0.0
        } else {
            self.fract_in_frame + processor.speed
        };

        let out_frame_to_in_frame = |out_frame: f64, in_frame_start: f64, speed: f64| -> f64 {
            in_frame_start + (out_frame * speed)
        };

        // The function which maps the output frame to the input frame is given by
        // the kinematic equation:
        //
        // in_frame = in_frame_start + (out_frame * start_speed) + (0.5 * accel * out_frame^2)
        //      where: accel = (end_speed - start_speed)
        let out_frame_to_in_frame_with_accel =
            |out_frame: f64, in_frame_start: f64, start_speed: f64, half_accel: f64| -> f64 {
                in_frame_start + (out_frame * start_speed) + (out_frame * out_frame * half_accel)
            };

        let num_channels = processor.num_channels_filled(out_buffers.len());
        let copy_start = if self.is_first_process { 0 } else { 2 };
        let mut finished_playing = false;

        if self.prev_speed == processor.speed {
            self.resample_linear_inner(
                out_frame_to_in_frame,
                in_frame_start,
                self.prev_speed,
                out_buffer_range.clone(),
                processor,
                extra,
                looping,
                copy_start,
                num_channels,
                out_buffers,
                out_buffer_range.start,
                &mut finished_playing,
            );
        } else {
            let half_accel = 0.5 * (processor.speed - self.prev_speed) / total_out_frames as f64;

            self.resample_linear_inner(
                |out_frame: f64, in_frame_start: f64, speed: f64| {
                    out_frame_to_in_frame_with_accel(out_frame, in_frame_start, speed, half_accel)
                },
                in_frame_start,
                self.prev_speed,
                out_buffer_range.clone(),
                processor,
                extra,
                looping,
                copy_start,
                num_channels,
                out_buffers,
                out_buffer_range.start,
                &mut finished_playing,
            );
        }

        self.prev_speed = processor.speed;
        self.is_first_process = false;

        (finished_playing, num_channels)
    }

    fn resample_linear_inner<OutToInFrame>(
        &mut self,
        out_to_in_frame: OutToInFrame,
        in_frame_start: f64,
        speed: f64,
        out_buffer_range: Range<usize>,
        processor: &mut SamplerProcessor,
        extra: &mut ProcExtra,
        looping: bool,
        mut copy_start: usize,
        num_channels: usize,
        out_buffers: &mut [&mut [f32]],
        out_buffer_start: usize,
        finished_playing: &mut bool,
    ) where
        OutToInFrame: Fn(f64, f64, f64) -> f64,
    {
        let mut scratch_buffers = extra.scratch_buffers.all_mut();

        let total_out_frames = out_buffer_range.end - out_buffer_range.start;
        let output_frame_end = (total_out_frames - 1) as f64;

        let input_frame_end = out_to_in_frame(output_frame_end, in_frame_start, speed);
        let input_frames_needed = input_frame_end.trunc() as usize + 2;

        let mut input_frames_processed = 0;
        let mut output_frames_processed = 0;
        while output_frames_processed < total_out_frames {
            let input_frames =
                (input_frames_needed - input_frames_processed).min(processor.max_block_frames);

            if input_frames > copy_start {
                let (finished, _) = processor.copy_from_sample(
                    &mut scratch_buffers[..num_channels],
                    copy_start..input_frames,
                    looping,
                );
                if finished {
                    *finished_playing = true;
                }
            }

            let max_block_frames_minus_1 = processor.max_block_frames - 1;
            let out_ch_start = out_buffer_start + output_frames_processed;

            let mut out_frames_count = 0;

            // Have an optimized loop for stereo audio.
            if num_channels == 2 {
                let mut last_in_frame = 0;
                let mut last_fract_frame = 0.0;

                let (out_ch_0, out_ch_1) = out_buffers.split_first_mut().unwrap();
                let (r_ch_0, r_ch_1) = scratch_buffers.split_first_mut().unwrap();

                let out_ch_0 = &mut out_ch_0[out_ch_start..out_buffer_range.end];
                let out_ch_1 = &mut out_ch_1[0][out_ch_start..out_buffer_range.end];

                let r_ch_0 = &mut r_ch_0[..processor.max_block_frames];
                let r_ch_1 = &mut r_ch_1[0][..processor.max_block_frames];

                if copy_start > 0 {
                    r_ch_0[0] = self.wraparound_buffer[0][0];
                    r_ch_1[0] = self.wraparound_buffer[1][0];

                    r_ch_0[1] = self.wraparound_buffer[0][1];
                    r_ch_1[1] = self.wraparound_buffer[1][1];
                }

                for (i, (out_s_0, out_s_1)) in
                    out_ch_0.iter_mut().zip(out_ch_1.iter_mut()).enumerate()
                {
                    let out_frame = (i + output_frames_processed) as f64;

                    let in_frame_f64 = out_to_in_frame(out_frame, in_frame_start, speed);

                    let in_frame_usize = in_frame_f64.trunc() as usize - input_frames_processed;
                    let fract_frame = in_frame_f64.fract();

                    if in_frame_usize >= max_block_frames_minus_1 {
                        break;
                    }

                    let s0_0 = r_ch_0[in_frame_usize];
                    let s0_1 = r_ch_1[in_frame_usize];

                    let s1_0 = r_ch_0[in_frame_usize + 1];
                    let s1_1 = r_ch_1[in_frame_usize + 1];

                    *out_s_0 = s0_0 + ((s1_0 - s0_0) * fract_frame as f32);
                    *out_s_1 = s0_1 + ((s1_1 - s0_1) * fract_frame as f32);

                    last_in_frame = in_frame_usize;
                    last_fract_frame = fract_frame;

                    out_frames_count += 1;
                }

                self.wraparound_buffer[0][0] = r_ch_0[last_in_frame];
                self.wraparound_buffer[1][0] = r_ch_1[last_in_frame];

                self.wraparound_buffer[0][1] = r_ch_0[last_in_frame + 1];
                self.wraparound_buffer[1][1] = r_ch_1[last_in_frame + 1];

                self.fract_in_frame = last_fract_frame;
            } else {
                for ((out_ch, r_ch), w_ch) in out_buffers[..num_channels]
                    .iter_mut()
                    .zip(scratch_buffers[..num_channels].iter_mut())
                    .zip(self.wraparound_buffer[..num_channels].iter_mut())
                {
                    // Hint to compiler to optimize loop.
                    assert_eq!(r_ch.len(), processor.max_block_frames);

                    if copy_start > 0 {
                        r_ch[0] = w_ch[0];
                        r_ch[1] = w_ch[1];
                    }

                    let mut last_in_frame = 0;
                    let mut last_fract_frame = 0.0;
                    let mut out_frames_ch_count = 0;
                    for (i, out_s) in out_ch[out_ch_start..out_buffer_range.end]
                        .iter_mut()
                        .enumerate()
                    {
                        let out_frame = (i + output_frames_processed) as f64;

                        let in_frame_f64 = out_to_in_frame(out_frame, in_frame_start, speed);

                        let in_frame_usize = in_frame_f64.trunc() as usize - input_frames_processed;
                        last_fract_frame = in_frame_f64.fract();

                        if in_frame_usize >= max_block_frames_minus_1 {
                            break;
                        }

                        let s0 = r_ch[in_frame_usize];
                        let s1 = r_ch[in_frame_usize + 1];

                        *out_s = s0 + ((s1 - s0) * last_fract_frame as f32);

                        last_in_frame = in_frame_usize;
                        out_frames_ch_count += 1;
                    }

                    w_ch[0] = r_ch[last_in_frame];
                    w_ch[1] = r_ch[last_in_frame + 1];

                    self.fract_in_frame = last_fract_frame;
                    out_frames_count = out_frames_ch_count;
                }
            }

            output_frames_processed += out_frames_count;
            input_frames_processed += input_frames - 2;

            copy_start = 2;
        }
    }

    pub fn reset(&mut self) {
        self.fract_in_frame = 0.0;
        self.is_first_process = true;
    }
}
