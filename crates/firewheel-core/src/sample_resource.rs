use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

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
    /// * `buffers` - The buffers to fill with data. If the length of `buffers`
    ///   is greater than the number of channels in this resource, then ignore
    ///   the extra buffers.
    /// * `buffer_range` - The range inside each buffer slice in which to
    ///   fill with data. Do not fill any data outside of this range.
    /// * `start_frame` - The sample (of a single channel of audio) in the
    ///   resource at which to start copying from. Not to be confused with video
    ///   frames.
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

#[derive(Clone)]
pub struct InterleavedResourceI16 {
    pub data: Vec<i16>,
    pub channels: NonZeroUsize,
    pub sample_rate: Option<NonZeroU32>,
}

impl SampleResourceInfo for InterleavedResourceI16 {
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

impl core::fmt::Debug for InterleavedResourceI16 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "InterleavedResourceI16 {{ channels: {}, frames: {} }}",
            self.channels.get(),
            self.data.len() / self.channels.get(),
        )
    }
}

#[derive(Clone)]
pub struct InterleavedResourceU16 {
    pub data: Vec<u16>,
    pub channels: NonZeroUsize,
    pub sample_rate: Option<NonZeroU32>,
}

impl SampleResourceInfo for InterleavedResourceU16 {
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

impl core::fmt::Debug for InterleavedResourceU16 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "InterleavedResourceU16 {{ channels: {}, frames: {} }}",
            self.channels.get(),
            self.data.len() / self.channels.get(),
        )
    }
}

#[derive(Clone)]
pub struct InterleavedResourceF32 {
    pub data: Vec<f32>,
    pub channels: NonZeroUsize,
    pub sample_rate: Option<NonZeroU32>,
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
    f32::from(s) * (1.0 / i16::MAX as f32)
}

#[inline]
pub fn pcm_u16_to_f32(s: u16) -> f32 {
    ((f32::from(s)) * (2.0 / u16::MAX as f32)) - 1.0
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
    for (buf, ch) in buffers.iter_mut().zip(data.iter()) {
        buf[buffer_range.clone()].copy_from_slice(
            &ch.as_ref()[start_frame..start_frame + buffer_range.end - buffer_range.start],
        );
    }
}
