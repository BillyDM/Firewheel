use std::{num::NonZeroUsize, ops::Range};

use firewheel_core::{
    collector::ArcGc,
    sample_resource::{SampleResource, SampleResourceInfo},
};

/// A wrapper around [`symphonium::DecodedAudio`] which implements the
/// [`SampleResource`] trait.
#[derive(Debug, Clone)]
pub struct DecodedAudio(pub symphonium::DecodedAudio);

impl DecodedAudio {
    pub fn duration_seconds(&self) -> f64 {
        self.0.frames() as f64 / self.0.sample_rate() as f64
    }

    pub fn into_dyn_resource(self) -> ArcGc<dyn SampleResource> {
        ArcGc::new_unsized(|| {
            bevy_platform::sync::Arc::new(self) as bevy_platform::sync::Arc<dyn SampleResource>
        })
    }
}

impl From<DecodedAudio> for ArcGc<dyn SampleResource> {
    fn from(value: DecodedAudio) -> Self {
        value.into_dyn_resource()
    }
}

impl SampleResourceInfo for DecodedAudio {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
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
    pub fn duration_seconds(&self, sample_rate: u32) -> f64 {
        self.0.frames() as f64 / sample_rate as f64
    }
}

impl SampleResourceInfo for DecodedAudioF32 {
    fn num_channels(&self) -> NonZeroUsize {
        NonZeroUsize::new(self.0.channels()).unwrap()
    }

    fn len_frames(&self) -> u64 {
        self.0.frames() as u64
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
pub fn load_audio_file<P: AsRef<std::path::Path>>(
    loader: &mut symphonium::SymphoniumLoader,
    path: P,
    #[cfg(feature = "resample")] sample_rate: core::num::NonZeroU32,
    #[cfg(feature = "resample")] resample_quality: symphonium::ResampleQuality,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load(
            path,
            #[cfg(feature = "resample")]
            Some(sample_rate.get()),
            #[cfg(feature = "resample")]
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
pub fn load_audio_file_from_source(
    loader: &mut symphonium::SymphoniumLoader,
    source: Box<dyn symphonium::symphonia::core::io::MediaSource>,
    hint: Option<symphonium::symphonia::core::probe::Hint>,
    #[cfg(feature = "resample")] sample_rate: core::num::NonZeroU32,
    #[cfg(feature = "resample")] resample_quality: symphonium::ResampleQuality,
) -> Result<DecodedAudio, symphonium::error::LoadError> {
    loader
        .load_from_source(
            source,
            hint,
            #[cfg(feature = "resample")]
            Some(sample_rate.get()),
            #[cfg(feature = "resample")]
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
#[cfg(feature = "stretch")]
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
#[cfg(feature = "stretch")]
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

/// A helper method to convert a [`symphonium::DecodedAudio`] resource into
/// a [`SampleResource`].
pub fn decoded_to_resource(
    data: symphonium::DecodedAudio,
) -> bevy_platform::sync::Arc<dyn SampleResource> {
    bevy_platform::sync::Arc::new(DecodedAudio(data))
}

/// A helper method to convert a [`symphonium::DecodedAudioF32`] resource into
/// a [`SampleResource`].
pub fn decoded_f32_to_resource(
    data: symphonium::DecodedAudioF32,
) -> bevy_platform::sync::Arc<dyn SampleResource> {
    bevy_platform::sync::Arc::new(DecodedAudioF32(data))
}
