use core::{num::NonZeroUsize, ops::Range};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

/// Trait returning information about a resource of audio samples
pub trait SampleResourceInfo: Send + Sync + 'static {
    /// The number of channels in this resource.
    fn num_channels(&self) -> NonZeroUsize;

    /// The length of this resource in samples (of a single channel of audio).
    ///
    /// Not to be confused with video frames.
    fn len_frames(&self) -> u64;
}

/// A resource of audio samples.
pub trait SampleResource: SampleResourceInfo {
    /// Fill the given buffers with audio data starting from the given
    /// starting frame in the resource.
    ///
    /// * `buffers` - The buffers to fill with data. If the length of `buffers`
    /// is greater than the number of channels in this resource, then ignore
    /// the extra buffers.
    /// * `buffer_range` - The range inside each buffer slice in which to
    /// fill with data. Do not fill any data outside of this range.
    /// * `start_frame` - The sample (of a single channel of audio) in the
    /// resource at which to start copying from. Not to be confused with video
    /// frames.
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    );
}

/// A resource of audio samples stored as de-interleaved f32 values.
pub trait SampleResourceF32: SampleResourceInfo {
    /// Get the the buffer for a given channel.
    fn channel(&self, i: usize) -> Option<&[f32]>;
}

pub struct InterleavedResourceI16 {
    pub data: Vec<i16>,
    pub channels: NonZeroUsize,
}

impl SampleResourceInfo for InterleavedResourceI16 {
    fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    fn len_frames(&self) -> u64 {
        (self.data.len() / self.channels.get()) as u64
    }
}

impl SampleResource for InterleavedResourceI16 {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_interleaved(
            buffers,
            buffer_range,
            start_frame as usize,
            self.channels,
            &self.data,
            pcm_i16_to_f32,
        );
    }
}

pub struct InterleavedResourceU16 {
    pub data: Vec<u16>,
    pub channels: NonZeroUsize,
}

impl SampleResourceInfo for InterleavedResourceU16 {
    fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    fn len_frames(&self) -> u64 {
        (self.data.len() / self.channels.get()) as u64
    }
}

impl SampleResource for InterleavedResourceU16 {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_interleaved(
            buffers,
            buffer_range,
            start_frame as usize,
            self.channels,
            &self.data,
            pcm_u16_to_f32,
        );
    }
}

pub struct InterleavedResourceF32 {
    pub data: Vec<f32>,
    pub channels: NonZeroUsize,
}

impl SampleResourceInfo for InterleavedResourceF32 {
    fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    fn len_frames(&self) -> u64 {
        (self.data.len() / self.channels.get()) as u64
    }
}
impl SampleResource for InterleavedResourceF32 {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_interleaved(
            buffers,
            buffer_range,
            start_frame as usize,
            self.channels,
            &self.data,
            |s| s,
        );
    }
}

impl SampleResourceInfo for Vec<Vec<i16>> {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.len()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self[0].len() as u64
    }
}

impl SampleResource for Vec<Vec<i16>> {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_deinterleaved(
            buffers,
            buffer_range,
            start_frame as usize,
            self.as_slice(),
            pcm_i16_to_f32,
        );
    }
}

impl SampleResourceInfo for Vec<Vec<u16>> {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.len()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self[0].len() as u64
    }
}

impl SampleResource for Vec<Vec<u16>> {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_deinterleaved(
            buffers,
            buffer_range,
            start_frame as usize,
            self.as_slice(),
            pcm_u16_to_f32,
        );
    }
}

impl SampleResourceInfo for Vec<Vec<f32>> {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.len()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self[0].len() as u64
    }
}

impl SampleResource for Vec<Vec<f32>> {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_deinterleaved_f32(buffers, buffer_range, start_frame as usize, self);
    }
}

impl SampleResourceF32 for Vec<Vec<f32>> {
    fn channel(&self, i: usize) -> Option<&[f32]> {
        self.get(i).map(|data| data.as_slice())
    }
}

#[inline]
pub fn pcm_i16_to_f32(s: i16) -> f32 {
    f32::from(s) * (1.0 / core::i16::MAX as f32)
}

#[inline]
pub fn pcm_u16_to_f32(s: u16) -> f32 {
    ((f32::from(s)) * (2.0 / core::u16::MAX as f32)) - 1.0
}

/// A helper method to fill buffers from a resource of interleaved samples.
pub fn fill_buffers_interleaved<T: Clone + Copy>(
    buffers: &mut [&mut [f32]],
    buffer_range: Range<usize>,
    start_frame: usize,
    channels: NonZeroUsize,
    data: &[T],
    convert: impl Fn(T) -> f32,
) {
    let start_frame = start_frame as usize;
    let channels = channels.get();

    let frames = buffer_range.end - buffer_range.start;

    if channels == 1 {
        // Mono, no need to deinterleave.
        for (buf_s, &src_s) in buffers[0][buffer_range.clone()]
            .iter_mut()
            .zip(&data[start_frame..start_frame + frames])
        {
            *buf_s = convert(src_s);
        }
        return;
    }

    if channels == 2 && buffers.len() >= 2 {
        // Provide an optimized loop for stereo.
        let (buf0, buf1) = buffers.split_first_mut().unwrap();
        let buf0 = &mut buf0[buffer_range.clone()];
        let buf1 = &mut buf1[0][buffer_range.clone()];

        let src_slice = &data[start_frame * 2..(start_frame + frames) * 2];

        for (src_chunk, (buf0_s, buf1_s)) in src_slice
            .chunks_exact(2)
            .zip(buf0.iter_mut().zip(buf1.iter_mut()))
        {
            *buf0_s = convert(src_chunk[0]);
            *buf1_s = convert(src_chunk[1]);
        }

        return;
    }

    let src_slice = &data[start_frame * channels..(start_frame + frames) * channels];
    for (ch_i, buf_ch) in (0..channels).zip(buffers.iter_mut()) {
        for (src_chunk, buf_s) in src_slice
            .chunks_exact(channels)
            .zip(buf_ch[buffer_range.clone()].iter_mut())
        {
            *buf_s = convert(src_chunk[ch_i]);
        }
    }
}

/// A helper method to fill buffers from a resource of deinterleaved samples.
pub fn fill_buffers_deinterleaved<T: Clone + Copy, V: AsRef<[T]>>(
    buffers: &mut [&mut [f32]],
    buffer_range: Range<usize>,
    start_frame: usize,
    data: &[V],
    convert: impl Fn(T) -> f32,
) {
    let start_frame = start_frame as usize;
    let frames = buffer_range.end - buffer_range.start;

    if data.len() == 2 && buffers.len() >= 2 {
        // Provide an optimized loop for stereo.
        let (buf0, buf1) = buffers.split_first_mut().unwrap();
        let buf0 = &mut buf0[buffer_range.clone()];
        let buf1 = &mut buf1[0][buffer_range.clone()];
        let s0 = &data[0].as_ref()[start_frame..start_frame + frames];
        let s1 = &data[1].as_ref()[start_frame..start_frame + frames];

        for i in 0..frames {
            buf0[i] = convert(s0[i]);
            buf1[i] = convert(s1[i]);
        }

        return;
    }

    for (buf, ch) in buffers.iter_mut().zip(data.iter()) {
        for (buf_s, &ch_s) in buf[buffer_range.clone()]
            .iter_mut()
            .zip(ch.as_ref()[start_frame..start_frame + frames].iter())
        {
            *buf_s = convert(ch_s);
        }
    }
}

/// A helper method to fill buffers from a resource of deinterleaved `f32` samples.
pub fn fill_buffers_deinterleaved_f32<V: AsRef<[f32]>>(
    buffers: &mut [&mut [f32]],
    buffer_range: Range<usize>,
    start_frame: usize,
    data: &[V],
) {
    let start_frame = start_frame as usize;

    for (buf, ch) in buffers.iter_mut().zip(data.iter()) {
        buf[buffer_range.clone()].copy_from_slice(
            &ch.as_ref()[start_frame..start_frame + buffer_range.end - buffer_range.start],
        );
    }
}

#[cfg(feature = "symphonium")]
/// A wrapper around [`symphonium::DecodedAudio`] which implements the
/// [`SampleResource`] trait.
pub struct DecodedAudio(pub symphonium::DecodedAudio);

#[cfg(feature = "symphonium")]
impl DecodedAudio {
    pub fn duration_seconds(&self) -> f64 {
        self.0.frames() as f64 / self.0.sample_rate() as f64
    }

    pub fn into_dyn_resource(self) -> crate::collector::ArcGc<dyn SampleResource> {
        crate::collector::ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(self) as bevy_platform::sync::Arc<dyn SampleResource>
        })
    }
}

#[cfg(feature = "symphonium")]
impl From<DecodedAudio> for crate::collector::ArcGc<dyn SampleResource> {
    fn from(value: DecodedAudio) -> Self {
        value.into_dyn_resource()
    }
}

#[cfg(feature = "symphonium")]
impl SampleResourceInfo for DecodedAudio {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
    }
}

#[cfg(feature = "symphonium")]
impl SampleResource for DecodedAudio {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        let channels = self.0.channels().min(buffers.len());

        if channels == 2 {
            let (b1, b2) = buffers.split_first_mut().unwrap();

            self.0.fill_stereo(
                start_frame as usize,
                &mut b1[buffer_range.clone()],
                &mut b2[0][buffer_range.clone()],
            );
        } else {
            for (ch_i, b) in buffers[0..channels].iter_mut().enumerate() {
                self.0
                    .fill_channel(ch_i, start_frame as usize, &mut b[buffer_range.clone()])
                    .unwrap();
            }
        }
    }
}

#[cfg(feature = "symphonium")]
impl From<symphonium::DecodedAudio> for DecodedAudio {
    fn from(data: symphonium::DecodedAudio) -> Self {
        Self(data)
    }
}

#[cfg(feature = "symphonium")]
/// A wrapper around [`symphonium::DecodedAudioF32`] which implements the
/// [`SampleResource`] trait.
pub struct DecodedAudioF32(pub symphonium::DecodedAudioF32);

#[cfg(feature = "symphonium")]
impl DecodedAudioF32 {
    pub fn duration_seconds(&self, sample_rate: u32) -> f64 {
        self.0.frames() as f64 / sample_rate as f64
    }
}

#[cfg(feature = "symphonium")]
impl SampleResourceInfo for DecodedAudioF32 {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
    }
}

#[cfg(feature = "symphonium")]
impl SampleResource for DecodedAudioF32 {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        fill_buffers_deinterleaved_f32(buffers, buffer_range, start_frame as usize, &self.0.data);
    }
}

#[cfg(feature = "symphonium")]
impl From<symphonium::DecodedAudioF32> for DecodedAudioF32 {
    fn from(data: symphonium::DecodedAudioF32) -> Self {
        Self(data)
    }
}

/// A helper method to load an audio file from a path using Symphonium.
///
/// * `loader` - The symphonium loader.
/// * `path`` - The path to the audio file stored on disk.
/// * `sample_rate` - The sample rate of the audio stream.
/// * `resample_quality` - The quality of the resampler to use.
#[cfg(feature = "symphonium")]
pub fn load_audio_file<P: AsRef<std::path::Path>>(
    loader: &mut symphonium::SymphoniumLoader,
    path: P,
    #[cfg(feature = "symphonium_resample")] sample_rate: core::num::NonZeroU32,
    #[cfg(feature = "symphonium_resample")] resample_quality: symphonium::ResampleQuality,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load(
            path,
            #[cfg(feature = "symphonium_resample")]
            Some(sample_rate.get()),
            #[cfg(feature = "symphonium_resample")]
            resample_quality,
            None,
        )
        .map(|d| DecodedAudio(d))
}

/// A helper method to load an audio file from a custom source using Symphonium.
///
/// * `loader` - The symphonium loader.
/// * `source` - The audio source which implements the [`MediaSource`] trait.
/// * `hint` -  An optional hint to help the format registry guess what format reader is appropriate.
/// * `sample_rate` - The sample rate of the audio stream.
/// * `resample_quality` - The quality of the resampler to use.
///
/// [`MediaSource`]: symphonium::symphonia::core::io::MediaSource
#[cfg(feature = "symphonium")]
pub fn load_audio_file_from_source(
    loader: &mut symphonium::SymphoniumLoader,
    source: Box<dyn symphonium::symphonia::core::io::MediaSource>,
    hint: Option<symphonium::symphonia::core::probe::Hint>,
    #[cfg(feature = "symphonium_resample")] sample_rate: core::num::NonZeroU32,
    #[cfg(feature = "symphonium_resample")] resample_quality: symphonium::ResampleQuality,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load_from_source(
            source,
            hint,
            #[cfg(feature = "symphonium_resample")]
            Some(sample_rate.get()),
            #[cfg(feature = "symphonium_resample")]
            resample_quality,
            None,
        )
        .map(|d| DecodedAudio(d))
}

/// A helper method to load an audio file from a path using Symphonium. This
/// also stretches (pitch shifts) the sample by the given amount.
///
/// * `loader` - The symphonium loader.
/// * `path`` - The path to the audio file stored on disk.
/// * `sample_rate` - The sample rate of the audio stream.
/// * `stretch` - The amount of stretching (`new_length / old_length`). A value of `1.0` is no
/// change, a value less than `1.0` will increase the pitch & decrease the length, and a value
/// greater than `1.0` will decrease the pitch & increase the length. If a `target_sample_rate`
/// is given, then the final amount will automatically be adjusted to account for that.
#[cfg(feature = "symphonium_stretch")]
pub fn load_audio_file_stretched<P: AsRef<std::path::Path>>(
    loader: &mut symphonium::SymphoniumLoader,
    path: P,
    sample_rate: core::num::NonZeroU32,
    stretch: f64,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load_f32_stretched(path, stretch, Some(sample_rate.get()), None)
        .map(|d| DecodedAudio(d.into()))
}

/// A helper method to load an audio file from a custom source using Symphonium. This
/// also stretches (pitch shifts) the sample by the given amount.
///
/// * `loader` - The symphonium loader.
/// * `source` - The audio source which implements the [`symphonium::symphonia::core::io::MediaSource`]
/// trait.
/// * `hint` -  An optional hint to help the format registry guess what format reader is appropriate.
/// * `sample_rate` - The sample rate of the audio stream.
/// * `stretch` - The amount of stretching (`new_length / old_length`). A value of `1.0` is no
/// change, a value less than `1.0` will increase the pitch & decrease the length, and a value
/// greater than `1.0` will decrease the pitch & increase the length. If a `target_sample_rate`
/// is given, then the final amount will automatically be adjusted to account for that.
#[cfg(feature = "symphonium_stretch")]
pub fn load_audio_file_from_source_stretched(
    loader: &mut symphonium::SymphoniumLoader,
    source: Box<dyn symphonium::symphonia::core::io::MediaSource>,
    hint: Option<symphonium::symphonia::core::probe::Hint>,
    sample_rate: core::num::NonZeroU32,
    stretch: f64,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load_f32_from_source_stretched(source, hint, stretch, Some(sample_rate.get()), None)
        .map(|d| DecodedAudio(d.into()))
}

#[cfg(feature = "symphonium")]
/// A helper method to convert a [`symphonium::DecodedAudio`] resource into
/// a [`SampleResource`].
pub fn decoded_to_resource(
    data: symphonium::DecodedAudio,
) -> bevy_platform::sync::Arc<dyn SampleResource> {
    bevy_platform::sync::Arc::new(DecodedAudio(data))
}

#[cfg(feature = "symphonium")]
/// A helper method to convert a [`symphonium::DecodedAudioF32`] resource into
/// a [`SampleResource`].
pub fn decoded_f32_to_resource(
    data: symphonium::DecodedAudioF32,
) -> bevy_platform::sync::Arc<dyn SampleResource> {
    bevy_platform::sync::Arc::new(DecodedAudioF32(data))
}
