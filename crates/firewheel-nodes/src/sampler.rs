// TODO: The logic in this has become increadibly complex and error-prone. I plan
// on rewriting the sampler engine using a state machine.

use bevy_platform::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use bevy_platform::time::Instant;
use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};
use firewheel_core::diff::RealtimeClone;
use firewheel_core::log::RealtimeLogger;
use smallvec::SmallVec;

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount, NonZeroChannelCount},
    clock::InstantSeconds,
    collector::ArcGc,
    diff::{Diff, Notify, ParamPath, Patch},
    dsp::{
        buffer::InstanceBuffer,
        declick::{DeclickValues, Declicker, FadeType},
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    event::{NodeEventList, NodeEventType, ParamData},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus, NUM_SCRATCH_BUFFERS,
    },
    sample_resource::SampleResource,
    SilenceMask, StreamInfo,
};

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

pub const MAX_OUT_CHANNELS: usize = 8;
pub const DEFAULT_NUM_DECLICKERS: usize = 2;
pub const MIN_PLAYBACK_SPEED: f64 = 0.0000001;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct SamplerConfig {
    /// The number of channels in this node.
    pub channels: NonZeroChannelCount,
    /// If `true`, then mono samples will be converted to stereo during playback.
    ///
    /// By default this is set to `true`.
    pub mono_to_stereo: bool,
    /// The maximum number of "declickers" present on this node.
    /// The more declickers there are, the more samples that can be declicked
    /// when played in rapid succession. (Note more declickers will allocate
    /// more memory).
    ///
    /// By default this is set to `2`.
    pub num_declickers: u32,
    /// If true, then samples will be crossfaded when the playhead or sample is
    /// changed (if a sample was currently playing when the event was sent).
    ///
    /// By default this is set to `true`.
    pub crossfade_on_seek: bool,
    /// If the resutling amplitude of the volume is less than or equal to this
    /// value, then the amplitude will be clamped to `0.0` (silence).
    pub amp_epsilon: f32,
    /// The quality of the resampling algorithm used when changing the playback
    /// speed.
    pub speed_quality: PlaybackSpeedQuality,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            mono_to_stereo: true,
            num_declickers: DEFAULT_NUM_DECLICKERS as u32,
            crossfade_on_seek: true,
            amp_epsilon: DEFAULT_AMP_EPSILON,
            speed_quality: PlaybackSpeedQuality::default(),
        }
    }
}

/// The quality of the resampling algorithm used for changing the playback
/// speed of a sampler node.
#[non_exhaustive]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub enum PlaybackSpeedQuality {
    #[default]
    /// Low quality, fast performance. Recommended for most use cases.
    ///
    /// More specifically, this uses a linear resampling algorithm with no
    /// antialiasing filter.
    LinearFast,
    // TODO: more quality options
}

#[derive(Clone, Diff, Patch)]
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

    /// The current playback state.
    pub playback: Notify<PlaybackState>,

    /// How many times a sample should be repeated.
    pub repeat_mode: RepeatMode,

    /// The speed at which to play the sample at. `1.0` means to play the sound at
    /// its original speed, `< 1.0` means to play the sound slower (which will make
    /// it lower-pitched), and `> 1.0` means to play the sound faster (which will
    /// make it higher-pitched).
    pub speed: f64,
}

impl Default for SamplerNode {
    fn default() -> Self {
        Self {
            sample: None,
            volume: Volume::default(),
            playback: Default::default(),
            repeat_mode: RepeatMode::default(),
            speed: 1.0,
        }
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

    /// Returns an event type to sync the `playback` parameter.
    pub fn sync_playback_event(&self) -> NodeEventType {
        NodeEventType::Param {
            data: ParamData::any(self.playback.clone()),
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

    /// Play the sample in this node.
    ///
    /// If a sample is already playing, then it will restart from the given playhead.
    ///
    /// * `playhead` - The playhead to start playing from. If this is `None`, then
    /// the sample will start from the beginning.
    pub fn start_or_restart(&mut self, playhead: Option<Playhead>) {
        *self.playback = PlaybackState::Play {
            playhead: Some(playhead.unwrap_or_default()),
        };
    }

    /// Pause sample playback.
    pub fn pause(&mut self) {
        *self.playback = PlaybackState::Pause;
    }

    /// Start/resume sample playback.
    pub fn resume(&mut self) {
        *self.playback = PlaybackState::Play { playhead: None };
    }

    /// Stop sample playback.
    ///
    /// Calling [`SamplerNode::resume`] after this will restart the sample from
    /// the beginning.
    pub fn stop(&mut self) {
        *self.playback = PlaybackState::Stop;
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
    pub fn playhead_frames(&self) -> u64 {
        self.shared_state
            .sample_playhead_frames
            .load(Ordering::Relaxed)
    }

    /// Get the current position of the sample playhead in seconds.
    ///
    /// * `sample_rate` - The sample rate of the current audio stream.
    pub fn playhead_seconds(&self, sample_rate: NonZeroU32) -> f64 {
        self.playhead_frames() as f64 / sample_rate.get() as f64
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
    ) -> u64 {
        let frames = self.playhead_frames();

        let Some(update_instant) = update_instant else {
            return frames;
        };

        if self.shared_state.playing.load(Ordering::Relaxed) {
            frames
                + InstantSeconds(update_instant.elapsed().as_secs_f64())
                    .to_samples(sample_rate)
                    .0 as u64
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
    ) -> f64 {
        self.playhead_frames_corrected(update_instant, sample_rate) as f64
            / sample_rate.get() as f64
    }

    /// Returns `true` if the sample has either not started playing yet or has finished
    /// playing.
    pub fn stopped(&self) -> bool {
        self.shared_state.stopped.load(Ordering::Relaxed)
    }

    /// Manually set the shared `stopped` flag. This can be useful to account for the delay
    /// between sending a play event and the node's processor receiving that event.
    pub fn mark_stopped(&self, stopped: bool) {
        self.shared_state.stopped.store(stopped, Ordering::Release);
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
            let stopped = self.stopped();

            match *params.playback {
                PlaybackState::Stop => u64::MAX - 1,
                PlaybackState::Pause => {
                    if stopped {
                        u64::MAX - 2
                    } else {
                        u64::MAX - 3
                    }
                }
                PlaybackState::Play { .. } => {
                    let playhead_frames = self.playhead_frames();

                    if stopped {
                        if playhead_frames > 0 {
                            // Sequence has likely finished playing.
                            u64::MAX - 4
                        } else {
                            // Sequence has likely not started playing yet.
                            u64::MAX - 5
                        }
                    } else {
                        // The older the sample is, the better it is as a candidate to steal
                        // work from.
                        playhead_frames
                    }
                }
            }
        } else {
            u64::MAX
        }
    }
}

/// A parameter representing the current playback state of a sample.
#[derive(Default, Debug, Clone, Copy, PartialEq, RealtimeClone)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub enum PlaybackState {
    /// Stop the sample.
    ///
    /// If the sample is started again with `PlaybackState::Play { playhead: None}`,
    /// it will restart from the beginning.
    #[default]
    Stop,
    /// Pause the sample.
    ///
    /// If the sample is started again with `PlaybackState::Play { playhead: None}`,
    /// it will resume from where it left off.
    Pause,
    /// Play the sample.
    Play {
        /// If this is `None`, then one of the two will happen:
        /// * If the previous state was `PlaybackState::Stop`, then the sample will
        /// be restarted from the beginning.
        /// * If the previous state was `PlaybackState::Pause`, then the sample will
        /// resume where it left off.
        ///
        /// If this is `Some`, then the sample will jump to the given position
        /// when the event is recieved. If the scheduled instant of this
        /// `PlaybackState` event is in the past when this node receives this
        /// event, then the delay will automatically be accounted for.
        playhead: Option<Playhead>,
    },
}

impl PlaybackState {
    pub const RESUME: Self = Self::Play { playhead: None };
    pub const RESTART: Self = Self::Play {
        playhead: Some(Playhead::ZERO),
    };

    pub fn is_playing(&self) -> bool {
        if let PlaybackState::Play { .. } = self {
            true
        } else {
            false
        }
    }
}

/// The playhead of a sample.
#[derive(Debug, Clone, Copy, PartialEq, RealtimeClone)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub enum Playhead {
    /// The playhead in units of seconds.
    Seconds(f64),
    /// The playhead in units of frames (samples in a single channel of audio).
    Frames(u64),
}

impl Playhead {
    pub const ZERO: Self = Playhead::Frames(0);

    pub fn as_frames(&self, sample_rate: NonZeroU32) -> u64 {
        match *self {
            Self::Seconds(seconds) => {
                if seconds <= 0.0 {
                    0
                } else {
                    (seconds.floor() as u64 * sample_rate.get() as u64)
                        + (seconds.fract() * sample_rate.get() as f64).round() as u64
                }
            }
            Self::Frames(frames) => frames,
        }
    }
}

impl Default for Playhead {
    fn default() -> Self {
        Self::ZERO
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
            playback_state: *self.playback,
            #[cfg(feature = "scheduled_events")]
            queued_playback_instant: None,
            amp_epsilon: config.amp_epsilon,
            is_first_process: true,
            max_block_frames: cx.stream_info.max_block_frames.get() as usize,
        }
    }
}

pub struct SamplerProcessor {
    config: SamplerConfig,
    params: SamplerNode,
    shared_state: ArcGc<SharedState>,

    loaded_sample_state: Option<LoadedSampleState>,

    declicker: Declicker,

    playback_state: PlaybackState,

    stop_declicker_buffers: Option<InstanceBuffer<f32, MAX_OUT_CHANNELS>>,
    stop_declickers: SmallVec<[StopDeclickerState; DEFAULT_NUM_DECLICKERS]>,
    num_active_stop_declickers: usize,

    resampler: Option<Resampler>,
    speed: f64,

    #[cfg(feature = "scheduled_events")]
    queued_playback_instant: Option<EventInstant>,

    amp_epsilon: f32,

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
        declick_values: &DeclickValues,
        scratch_buffers: &mut [&mut [f32]; NUM_SCRATCH_BUFFERS],
    ) -> (bool, usize) {
        let (finished_playing, mut channels_filled) = if self.speed != 1.0 {
            // Get around borrow checker.
            let mut resampler = self.resampler.take().unwrap();

            let (finished_playing, channels_filled) =
                resampler.resample_linear(buffers, 0..frames, scratch_buffers, self, looping);

            self.resampler = Some(resampler);

            (finished_playing, channels_filled)
        } else {
            self.resampler.as_mut().unwrap().reset();

            self.copy_from_sample(buffers, 0..frames, looping)
        };

        let Some(state) = self.loaded_sample_state.as_ref() else {
            return (true, 0);
        };

        if !self.declicker.is_settled() {
            self.declicker.process(
                buffers,
                0..frames,
                declick_values,
                state.gain,
                FadeType::EqualPower3dB,
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
            self.playback_state.is_playing()
                || (self.playback_state == PlaybackState::Pause && !self.declicker.is_settled())
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

    fn stop(
        &mut self,
        declick_values: &DeclickValues,
        num_out_channels: usize,
        scratch_buffers: &mut [&mut [f32]; NUM_SCRATCH_BUFFERS],
    ) {
        if self.currently_processing_sample() {
            // Fade out the sample into a temporary look-ahead
            // buffer to declick.

            self.declicker.fade_to_0(declick_values);

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
                        .get_mut(declicker_i, n_channels, fade_out_frames)
                        .unwrap();

                    self.process_internal(
                        &mut tmp_buffers,
                        fade_out_frames,
                        false,
                        declick_values,
                        scratch_buffers,
                    );

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
        let mut gain = self.params.volume.amp_clamped(self.amp_epsilon);
        if gain > 0.99999 && gain < 1.00001 {
            gain = 1.0;
        }

        let sample_len_frames = sample.len_frames();
        let sample_num_channels = sample.num_channels();

        let sample_mono_to_stereo =
            self.config.mono_to_stereo && num_out_channels > 1 && sample_num_channels.get() == 1;

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
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        events: &mut NodeEventList,
        _logger: &mut RealtimeLogger,
    ) -> ProcessStatus {
        let mut sample_changed = self.is_first_process;
        let mut playback_changed = false;
        let mut repeat_mode_changed = false;
        let mut speed_changed = false;
        let mut volume_changed = false;

        #[cfg(feature = "scheduled_events")]
        let mut playback_instant: Option<EventInstant> = None;

        #[cfg(not(feature = "scheduled_events"))]
        for patch in events.drain_patches::<SamplerNode>() {
            match patch {
                SamplerNodePatch::Sample(_) => sample_changed = true,
                SamplerNodePatch::Volume(_) => volume_changed = true,
                SamplerNodePatch::Playback(_) => {
                    playback_changed = true;
                }
                SamplerNodePatch::RepeatMode(_) => repeat_mode_changed = true,
                SamplerNodePatch::Speed(_) => speed_changed = true,
            }

            self.params.apply(patch);
        }

        #[cfg(feature = "scheduled_events")]
        for (patch, timestamp) in events.drain_patches_with_timestamps::<SamplerNode>() {
            match patch {
                SamplerNodePatch::Sample(_) => sample_changed = true,
                SamplerNodePatch::Volume(_) => volume_changed = true,
                SamplerNodePatch::Playback(_) => {
                    playback_changed = true;
                    playback_instant = timestamp;
                }
                SamplerNodePatch::RepeatMode(_) => repeat_mode_changed = true,
                SamplerNodePatch::Speed(_) => speed_changed = true,
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
                loaded_sample.gain = self.params.volume.amp_clamped(self.amp_epsilon);
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
            if !playback_changed {
                playback_changed = true;

                #[cfg(feature = "scheduled_events")]
                if let Some(queued_playback_instant) = self.queued_playback_instant.take() {
                    if queued_playback_instant.to_samples(proc_info).is_some() {
                        playback_instant = Some(queued_playback_instant);
                    } else {
                        // Handle an edge case where the user sent a scheduled play event at
                        // a musical time, but there is no sample and a musical transport
                        // is not playing.
                        playback_changed = false;
                    }
                }
            }

            self.stop(
                proc_info.declick_values,
                buffers.outputs.len(),
                buffers.scratch_buffers,
            );

            self.loaded_sample_state = None;

            if let Some(sample) = &self.params.sample {
                self.load_sample(ArcGc::clone(sample), buffers.outputs.len());
            } else {
                self.shared_state.stopped.store(true, Ordering::Relaxed);
            }

            self.playback_state = PlaybackState::Stop;
        }

        if playback_changed {
            match *self.params.playback {
                PlaybackState::Stop => {
                    self.stop(
                        proc_info.declick_values,
                        buffers.outputs.len(),
                        buffers.scratch_buffers,
                    );

                    self.playback_state = PlaybackState::Stop;
                }
                PlaybackState::Pause => {
                    if self.playback_state.is_playing() {
                        self.playback_state = PlaybackState::Pause;

                        self.declicker.fade_to_0(proc_info.declick_values);
                    }
                }
                PlaybackState::Play { playhead } => {
                    if self.playback_state.is_playing() && playhead.is_none() {
                        // Sample is already playing, no need to do anything.
                        #[cfg(feature = "scheduled_events")]
                        {
                            self.queued_playback_instant = None;
                        }
                    } else if self.loaded_sample_state.is_some() {
                        let loaded_sample_state = self.loaded_sample_state.as_mut().unwrap();
                        let prev_playhead_frames = loaded_sample_state.playhead_frames;

                        if playhead.is_some() {
                            loaded_sample_state.num_times_looped_back = 0;
                        }

                        let playhead_frames_at_play_instant = playhead
                            .map(|p| p.as_frames(proc_info.sample_rate))
                            .unwrap_or_else(|| match self.playback_state {
                                PlaybackState::Stop => 0,
                                _ => prev_playhead_frames,
                            });

                        #[cfg(feature = "scheduled_events")]
                        let mut new_playhead_frames =
                            if let Some(playback_instant) = playback_instant {
                                let playback_instant_samples = playback_instant
                                    .to_samples(proc_info)
                                    .unwrap_or(proc_info.clock_samples);
                                let delay = if playback_instant_samples < proc_info.clock_samples {
                                    (proc_info.clock_samples - playback_instant_samples).0 as u64
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
                                    while new_playhead_frames
                                        >= loaded_sample_state.sample_len_frames
                                    {
                                        new_playhead_frames -=
                                            loaded_sample_state.sample_len_frames;
                                        loaded_sample_state.num_times_looped_back += 1;
                                    }
                                }
                                RepeatMode::RepeatMultiple {
                                    num_times_to_repeat,
                                } => {
                                    while new_playhead_frames
                                        >= loaded_sample_state.sample_len_frames
                                    {
                                        if loaded_sample_state.num_times_looped_back
                                            == num_times_to_repeat as u64
                                        {
                                            new_playhead_frames =
                                                loaded_sample_state.sample_len_frames;
                                            break;
                                        }

                                        new_playhead_frames -=
                                            loaded_sample_state.sample_len_frames;
                                        loaded_sample_state.num_times_looped_back += 1;
                                    }
                                }
                            }
                        }

                        if prev_playhead_frames != new_playhead_frames {
                            self.stop(
                                proc_info.declick_values,
                                buffers.outputs.len(),
                                buffers.scratch_buffers,
                            );

                            self.loaded_sample_state.as_mut().unwrap().playhead_frames =
                                new_playhead_frames;

                            self.shared_state
                                .sample_playhead_frames
                                .store(new_playhead_frames, Ordering::Relaxed);
                        }

                        if new_playhead_frames
                            == self.loaded_sample_state.as_ref().unwrap().sample_len_frames
                        {
                            self.playback_state = PlaybackState::Stop;
                        } else {
                            if new_playhead_frames != 0
                                || (self.num_active_stop_declickers > 0
                                    && self.config.crossfade_on_seek)
                            {
                                self.declicker.reset_to_0();
                                self.declicker.fade_to_1(proc_info.declick_values);
                            } else {
                                self.declicker.reset_to_1();
                            }

                            self.playback_state = PlaybackState::Play { playhead };
                        }

                        #[cfg(feature = "scheduled_events")]
                        {
                            self.queued_playback_instant = None;
                        }
                    } else {
                        self.playback_state = PlaybackState::Stop;

                        #[cfg(feature = "scheduled_events")]
                        {
                            self.queued_playback_instant = playback_instant;
                        }
                    }
                }
            }
        }

        self.is_first_process = false;

        self.shared_state
            .playing
            .store(self.playback_state.is_playing(), Ordering::Relaxed);
        self.shared_state.stopped.store(
            self.playback_state == PlaybackState::Stop,
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

            let (finished, n_channels) = self.process_internal(
                buffers.outputs,
                proc_info.frames,
                looping,
                proc_info.declick_values,
                buffers.scratch_buffers,
            );

            num_filled_channels = n_channels;

            self.shared_state.sample_playhead_frames.store(
                self.loaded_sample_state.as_ref().unwrap().playhead_frames,
                Ordering::Relaxed,
            );

            if finished {
                self.playback_state = PlaybackState::Stop;
                self.shared_state.stopped.store(true, Ordering::Relaxed);
                self.shared_state
                    .finished
                    .store(self.params.playback.id(), Ordering::Relaxed);
            }
        }

        for (i, out_buf) in buffers
            .outputs
            .iter_mut()
            .enumerate()
            .skip(num_filled_channels)
        {
            if !proc_info.out_silence_mask.is_channel_silent(i) {
                out_buf[..proc_info.frames].fill(0.0);
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
                    .get(declicker_i, declicker.channels, fade_out_frames)
                    .unwrap();

                let copy_frames = proc_info.frames.min(declicker.frames_left);
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

        ProcessStatus::OutputsModified { out_silence_mask }
    }

    fn new_stream(&mut self, stream_info: &StreamInfo) {
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
            self.playback_state = PlaybackState::Stop;
            self.shared_state.playing.store(false, Ordering::Relaxed);
            self.shared_state.stopped.store(true, Ordering::Relaxed);
            self.shared_state.finished.store(0, Ordering::Relaxed);
        }
    }
}

struct SharedState {
    sample_playhead_frames: AtomicU64,
    playing: AtomicBool,
    stopped: AtomicBool,
    finished: AtomicU64,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            sample_playhead_frames: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            stopped: AtomicBool::new(true),
            finished: AtomicU64::new(0),
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
        scratch_buffers: &mut [&mut [f32]; NUM_SCRATCH_BUFFERS],
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
                scratch_buffers,
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
                scratch_buffers,
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
        scratch_buffers: &mut [&mut [f32]; NUM_SCRATCH_BUFFERS],
        looping: bool,
        mut copy_start: usize,
        num_channels: usize,
        out_buffers: &mut [&mut [f32]],
        out_buffer_start: usize,
        finished_playing: &mut bool,
    ) where
        OutToInFrame: Fn(f64, f64, f64) -> f64,
    {
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
