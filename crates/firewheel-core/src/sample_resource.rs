use audioadapter::Adapter;
use audioadapter_buffers::{
    adapter_to_float::ConvertNumbers,
    direct::{InterleavedSlice, SequentialSlice},
};
use audioadapter_sample::sample::RawSample;
use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use crate::collector::ArcGc;

/// Trait returning information about a resource of audio samples
pub trait SampleResourceInfo {
    /// The number of channels in this resource.
    fn num_channels(&self) -> NonZeroUsize;

    /// The length of this resource in samples (of a single channel of audio).
    ///
    /// Not to be confused with video frames.
    fn len_frames(&self) -> u64;

    /// The sample rate of this resource.
    ///
    /// Returns `None` if the sample rate is unknown.
    fn sample_rate(&self) -> Option<NonZeroU32> {
        None
    }
}

/// A resource of audio samples.
pub trait SampleResource: SampleResourceInfo {
    /// Fill the given buffers with audio data starting from the given
    /// starting frame in the resource.
    ///
    /// * `out_buffer` - The buffers to fill with data. If the length of `buffers`
    ///   is greater than the number of channels in this resource, then ignore
    ///   the extra buffers.
    /// * `out_buffer_range` - The range inside each buffer slice in which to
    ///   fill with data. Do not fill any data outside of this range.
    /// * `start_frame` - The sample (of a single channel of audio) in the
    ///   resource at which to start copying from. Not to be confused with video
    ///   frames.
    ///
    /// If the length of `out_buffer_range` is all or partly out of bounds of
    /// the resource, then the frames which are out of bounds will be left
    /// untouched.
    ///
    /// Returns the number of frames that were successfully filled. This may
    /// be less than the length of `out_buffer_range` if the range is all or
    /// partly out of bounds of the resource
    fn fill_buffers(
        &self,
        out_buffer: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        start_frame: u64,
    ) -> usize;
}

/// A resource of audio samples stored as de-interleaved f32 values.
pub trait SampleResourceF32: SampleResourceInfo {
    /// Get the the buffer for a given channel.
    fn channel(&self, i: usize) -> Option<&[f32]>;
}

impl<T: SampleResource + Send + Sync + 'static> From<T>
    for ArcGc<dyn SampleResource + Send + Sync + 'static>
{
    fn from(value: T) -> Self {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(value)
                as bevy_platform::sync::Arc<dyn SampleResource + Send + Sync + 'static>
        })
    }
}

impl<T: SampleResourceF32 + Send + Sync + 'static> From<T>
    for ArcGc<dyn SampleResourceF32 + Send + Sync + 'static>
{
    fn from(value: T) -> Self {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(value)
                as bevy_platform::sync::Arc<dyn SampleResourceF32 + Send + Sync + 'static>
        })
    }
}

#[derive(Clone)]
pub struct InterleavedResourceF32 {
    pub data: Vec<f32>,
    pub channels: NonZeroUsize,
    pub sample_rate: Option<NonZeroU32>,
}

impl InterleavedResourceF32 {
    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResource + Send + Sync + 'static> {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(self)
                as bevy_platform::sync::Arc<dyn SampleResource + Send + Sync + 'static>
        })
    }
}

impl SampleResourceInfo for InterleavedResourceF32 {
    fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    fn len_frames(&self) -> u64 {
        (self.data.len() / self.channels.get()) as u64
    }

    fn sample_rate(&self) -> Option<NonZeroU32> {
        self.sample_rate
    }
}

impl SampleResource for InterleavedResourceF32 {
    fn fill_buffers(
        &self,
        out_buffer: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        start_frame: u64,
    ) -> usize {
        fill_buffers_interleaved(
            out_buffer,
            out_buffer_range,
            start_frame,
            self.channels,
            &self.data,
            self.len_frames() as usize,
        )
    }
}

impl core::fmt::Debug for InterleavedResourceF32 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "InterleavedResourceF32 {{ channels: {}, frames: {} }}",
            self.channels.get(),
            self.data.len() / self.channels.get(),
        )
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
        out_buffer: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        start_frame: u64,
    ) -> usize {
        fill_buffers_deinterleaved_f32(
            out_buffer,
            out_buffer_range,
            start_frame,
            self,
            self[0].len(),
        )
    }
}

impl SampleResourceF32 for Vec<Vec<f32>> {
    fn channel(&self, i: usize) -> Option<&[f32]> {
        self.get(i).map(|data| data.as_slice())
    }
}

/// A helper method to fill buffers from a resource of interleaved samples.
///
/// Returns the number of frames that were successfully filled. This may
/// be less than the length of `out_buffer_range` if the range is all or
/// partly out of bounds of the resource
pub fn fill_buffers_interleaved<T: RawSample + Clone>(
    out_buffer: &mut [&mut [f32]],
    out_buffer_range: Range<usize>,
    start_frame: u64,
    channels: NonZeroUsize,
    resource: &[T],
    resource_len_frames: usize,
) -> usize {
    let channels = channels.get();

    let Some((frames, start_frame)) = constrain_frames(
        out_buffer_range.end - out_buffer_range.start,
        start_frame,
        resource_len_frames,
    ) else {
        return 0;
    };

    // Provide an optimized loop for stereo.
    if channels == 2 && out_buffer.len() >= 2 {
        let (buf0, buf1) = out_buffer.split_first_mut().unwrap();
        let buf0 = &mut buf0[out_buffer_range.start..out_buffer_range.start + frames];
        let buf1 = &mut buf1[0][out_buffer_range.start..out_buffer_range.start + frames];

        let src_slice = &resource[start_frame * 2..(start_frame + frames) * 2];

        for (src_chunk, (buf0_s, buf1_s)) in src_slice
            .chunks_exact(2)
            .zip(buf0.iter_mut().zip(buf1.iter_mut()))
        {
            *buf0_s = src_chunk[0].to_scaled_float();
            *buf1_s = src_chunk[1].to_scaled_float();
        }

        return frames;
    }

    let src_slice = &resource[start_frame * channels..(start_frame + frames) * channels];

    let adapter = InterleavedSlice::new(src_slice, channels, frames).unwrap();
    let convert = ConvertNumbers::<&dyn Adapter<T>, f32>::new(&adapter as &dyn Adapter<T>);

    for (ch_i, out_ch) in out_buffer.iter_mut().enumerate().take(channels) {
        convert.copy_from_channel_to_slice(
            ch_i,
            start_frame,
            &mut out_ch[out_buffer_range.start..out_buffer_range.start + frames],
        );
    }

    frames
}

/// A helper method to fill buffers from a resource of deinterleaved samples.
///
/// Returns the number of frames that were successfully filled. This may
/// be less than the length of `out_buffer_range` if the range is all or
/// partly out of bounds of the resource
pub fn fill_channel_deinterleaved<T: RawSample + Clone>(
    out_buffer_channel: &mut [f32],
    out_buffer_range: Range<usize>,
    start_frame: u64,
    resource_channel: &[T],
) -> usize {
    let Some((frames, start_frame)) = constrain_frames(
        out_buffer_range.end - out_buffer_range.start,
        start_frame,
        resource_channel.len(),
    ) else {
        return 0;
    };

    let adapter = SequentialSlice::new(resource_channel, 1, frames).unwrap();
    let convert = ConvertNumbers::<&dyn Adapter<T>, f32>::new(&adapter as &dyn Adapter<T>);

    convert.copy_from_channel_to_slice(
        0,
        start_frame,
        &mut out_buffer_channel[out_buffer_range.start..out_buffer_range.start + frames],
    );

    frames
}

/// A helper method to fill buffers from a resource of deinterleaved `f32` samples.
///
/// Returns the number of frames that were successfully filled. This may
/// be less than the length of `out_buffer_range` if the range is all or
/// partly out of bounds of the resource
pub fn fill_buffers_deinterleaved_f32<V: AsRef<[f32]>>(
    out_buffer: &mut [&mut [f32]],
    out_buffer_range: Range<usize>,
    start_frame: u64,
    resource_channels: &[V],
    resource_len_frames: usize,
) -> usize {
    let Some((frames, start_frame)) = constrain_frames(
        out_buffer_range.end - out_buffer_range.start,
        start_frame,
        resource_len_frames,
    ) else {
        return 0;
    };

    for (out_ch, in_ch) in out_buffer.iter_mut().zip(resource_channels.iter()) {
        out_ch[out_buffer_range.start..out_buffer_range.start + frames]
            .copy_from_slice(&in_ch.as_ref()[start_frame..start_frame + frames]);
    }

    frames
}

/// A helper to constrain the requested number of frames to the available frames
/// in the sample resource.
///
/// Returns `Some((available_frames, start_frame as usize))` if the range is all
/// or partly contained in the resource, or `None` if the range is fully outside
/// the resource (`available_frames == 0`).
pub fn constrain_frames(
    requested_frames: usize,
    start_frame: u64,
    resource_len_frames: usize,
) -> Option<(usize, usize)> {
    let frames = (requested_frames as u64)
        .min((resource_len_frames as u64).saturating_sub(start_frame)) as usize;
    if frames == 0 {
        None
    } else {
        Some((frames, start_frame as usize))
    }
}
