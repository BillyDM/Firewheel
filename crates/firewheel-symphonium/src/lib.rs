use core::{
    num::{NonZeroU32, NonZeroUsize},
    ops::{Index, IndexMut, Range},
};

use firewheel_core::{
    collector::ArcGc,
    sample_resource::{SampleResource, SampleResourceF32, SampleResourceInfo},
};

/// A wrapper around [`symphonium::DecodedAudio`] which implements the
/// [`SampleResource`] trait.
#[derive(Debug, Clone)]
pub struct SymphoniumAudio(pub symphonium::DecodedAudio);

impl SymphoniumAudio {
    pub fn duration_seconds(&self) -> f64 {
        self.0.frames() as f64 / self.0.sample_rate().get() as f64
    }

    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResource> {
        self.into()
    }

    /// The sample rate of this resource.
    pub fn sample_rate(&self) -> NonZeroU32 {
        self.0.sample_rate()
    }

    /// The sample rate of the audio resource before it was resampled (if it was resampled).
    pub fn original_sample_rate(&self) -> NonZeroU32 {
        self.0.original_sample_rate()
    }
}

impl SampleResourceInfo for SymphoniumAudio {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
    }

    fn sample_rate(&self) -> Option<NonZeroU32> {
        Some(self.0.sample_rate())
    }
}

impl SampleResource for SymphoniumAudio {
    fn fill_buffers(
        &self,
        out_buffer: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        if start_frame > self.0.frames() as u64 {
            return;
        }
        let start_frame = start_frame as usize;

        for (ch_i, out_ch) in out_buffer.iter_mut().enumerate().take(self.0.channels()) {
            self.0
                .fill_channel(ch_i, start_frame, &mut out_ch[out_buffer_range.clone()])
                .unwrap();
        }
    }
}

impl From<symphonium::DecodedAudio> for SymphoniumAudio {
    fn from(data: symphonium::DecodedAudio) -> Self {
        Self(data)
    }
}

/// A wrapper around [`symphonium::DecodedAudioF32`] which implements the
/// [`SampleResource`] trait.
#[derive(Debug, Clone)]
pub struct SymphoniumAudioF32(pub symphonium::DecodedAudioF32);

impl SymphoniumAudioF32 {
    pub fn duration_seconds(&self, sample_rate: NonZeroU32) -> f64 {
        self.0.frames() as f64 / sample_rate.get() as f64
    }

    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResourceF32> {
        self.into()
    }

    /// The sample rate of this resource.
    pub fn sample_rate(&self) -> NonZeroU32 {
        self.0.sample_rate
    }

    /// The sample rate of the audio resource before it was resampled (if it was resampled).
    pub fn original_sample_rate(&self) -> NonZeroU32 {
        self.0.original_sample_rate
    }
}

impl Index<usize> for SymphoniumAudioF32 {
    type Output = Vec<f32>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0.data[index]
    }
}

impl IndexMut<usize> for SymphoniumAudioF32 {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0.data[index]
    }
}

impl SampleResourceInfo for SymphoniumAudioF32 {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
    }

    fn sample_rate(&self) -> Option<NonZeroU32> {
        Some(self.0.sample_rate)
    }
}

impl SampleResource for SymphoniumAudioF32 {
    fn fill_buffers(
        &self,
        out_buffer: &mut [&mut [f32]],
        out_buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        firewheel_core::sample_resource::fill_buffers_deinterleaved_f32(
            out_buffer,
            out_buffer_range,
            start_frame,
            &self.0.data,
            self.0.frames(),
        );
    }
}

impl SampleResourceF32 for SymphoniumAudioF32 {
    fn channel(&self, i: usize) -> Option<&[f32]> {
        self.0.data.get(i).map(|ch| ch.as_slice())
    }
}

impl From<symphonium::DecodedAudioF32> for SymphoniumAudioF32 {
    fn from(data: symphonium::DecodedAudioF32) -> Self {
        Self(data)
    }
}

/// A helper method to convert a [`symphonium::DecodedAudio`] resource into
/// a type erased [`SampleResource`].
pub fn dyn_symphonium_resource(data: symphonium::DecodedAudio) -> ArcGc<dyn SampleResource> {
    SymphoniumAudio(data).into_dyn_resource()
}

/// A helper method to convert a [`symphonium::DecodedAudioF32`] resource into
/// a type erased [`SampleResource`].
pub fn dyn_symphonium_resource_f32(
    data: symphonium::DecodedAudioF32,
) -> ArcGc<dyn SampleResourceF32> {
    SymphoniumAudioF32(data).into_dyn_resource()
}
