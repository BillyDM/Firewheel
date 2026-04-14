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
pub struct DecodedAudio(pub symphonium::DecodedAudio);

impl DecodedAudio {
    pub fn duration_seconds(&self) -> f64 {
        self.0.frames() as f64 / self.0.sample_rate().get() as f64
    }

    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResource> {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(self) as bevy_platform::sync::Arc<dyn SampleResource>
        })
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

impl SampleResourceInfo for DecodedAudio {
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

impl From<symphonium::DecodedAudio> for DecodedAudio {
    fn from(data: symphonium::DecodedAudio) -> Self {
        Self(data)
    }
}

/// A wrapper around [`symphonium::DecodedAudioF32`] which implements the
/// [`SampleResource`] trait.
#[derive(Debug, Clone)]
pub struct DecodedAudioF32(pub symphonium::DecodedAudioF32);

impl DecodedAudioF32 {
    pub fn duration_seconds(&self, sample_rate: NonZeroU32) -> f64 {
        self.0.frames() as f64 / sample_rate.get() as f64
    }

    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResourceF32> {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(self) as bevy_platform::sync::Arc<dyn SampleResourceF32>
        })
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

impl Index<usize> for DecodedAudioF32 {
    type Output = Vec<f32>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0.data[index]
    }
}

impl IndexMut<usize> for DecodedAudioF32 {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0.data[index]
    }
}

impl SampleResourceInfo for DecodedAudioF32 {
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

impl SampleResource for DecodedAudioF32 {
    fn fill_buffers(
        &self,
        buffers: &mut [&mut [f32]],
        buffer_range: Range<usize>,
        start_frame: u64,
    ) {
        firewheel_core::sample_resource::fill_buffers_deinterleaved_f32(
            buffers,
            buffer_range,
            start_frame as usize,
            &self.0.data,
        );
    }
}

impl SampleResourceF32 for DecodedAudioF32 {
    fn channel(&self, i: usize) -> Option<&[f32]> {
        self.0.data.get(i).map(|ch| ch.as_slice())
    }
}

impl From<symphonium::DecodedAudioF32> for DecodedAudioF32 {
    fn from(data: symphonium::DecodedAudioF32) -> Self {
        Self(data)
    }
}

/// A helper method to convert a [`symphonium::DecodedAudio`] resource into
/// a [`SampleResource`].
pub fn dyn_sample_resource(data: symphonium::DecodedAudio) -> ArcGc<dyn SampleResource> {
    DecodedAudio(data).into_dyn_resource()
}

/// A helper method to convert a [`symphonium::DecodedAudioF32`] resource into
/// a [`SampleResource`].
pub fn dyn_sample_resource_f32(data: symphonium::DecodedAudioF32) -> ArcGc<dyn SampleResourceF32> {
    DecodedAudioF32(data).into_dyn_resource()
}
