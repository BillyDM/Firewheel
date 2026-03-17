use core::num::NonZeroUsize;

use arrayvec::ArrayVec;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

/// A memory-efficient buffer of samples with `CHANNELS` channels. Each channel
/// has a length of `frames`.
///
/// `T` is the backing type of the storage, typically f32.
///
/// This is like a [`SequentialBuffer`] but guarantees all `MAX_CHANNELS` are present.
///
/// The number of frames and number of channels cannot be changed once constructed.
#[derive(Debug)]
pub struct ConstSequentialBuffer<T: Clone + Copy + Default, const CHANNELS: usize> {
    buffer: Vec<T>,
    num_frames: usize,
}

impl<T: Clone + Copy + Default, const CHANNELS: usize> ConstSequentialBuffer<T, CHANNELS> {
    pub const fn empty() -> Self {
        assert!(CHANNELS > 0);

        Self {
            buffer: Vec::new(),
            num_frames: 0,
        }
    }

    pub fn new(frames: usize) -> Self {
        assert!(CHANNELS > 0);

        let buffer_len = frames * CHANNELS;

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self {
            buffer,
            num_frames: frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.num_frames
    }

    /// Get an immutable reference to the first channel.
    #[inline]
    pub fn first(&self) -> &[T] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        unsafe { core::slice::from_raw_parts(self.buffer.as_ptr(), self.num_frames) }
    }

    /// Get a mutable reference to the first channel.
    #[inline]
    pub fn first_mut(&mut self) -> &mut [T] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        // * `self` is borrowed mutably in this method, so all mutability rules are
        // being upheld.
        unsafe { core::slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), self.num_frames) }
    }

    /// Get an immutable reference to the first channel with the given number of
    /// frames.
    ///
    /// The length of the returned slice will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    #[inline]
    pub fn first_with_frames(&self, frames: usize) -> &[T] {
        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        unsafe { core::slice::from_raw_parts(self.buffer.as_ptr(), frames) }
    }

    /// Get a mutable reference to the first channel with the given number of
    /// frames.
    ///
    /// The length of the returned slice will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    #[inline]
    pub fn first_with_frames_mut(&mut self, frames: usize) -> &mut [T] {
        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        // * `self` is borrowed mutably in this method, so all mutability rules are
        // being upheld.
        unsafe { core::slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), frames) }
    }

    /// Get an immutable reference to the first given number of channels in this buffer.
    ///
    /// # Panics
    /// Panics if `NUM_CHANNELS > Self::CHANNELS`
    pub fn channels<const NUM_CHANNELS: usize>(&self) -> [&[T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS above, so this is always within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.num_frames),
                    self.num_frames,
                )
            })
        }
    }

    /// Get a mutable reference to the first given number of channels in this buffer.
    ///
    /// # Panics
    /// Panics if `NUM_CHANNELS > Self::CHANNELS`
    pub fn channels_mut<const NUM_CHANNELS: usize>(&mut self) -> [&mut [T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS above, so this is always within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.num_frames),
                    self.num_frames,
                )
            })
        }
    }

    /// Get an immutable reference to the first given number of channels with the
    /// given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    ///
    /// # Panics
    /// Panics if `NUM_CHANNELS > Self::CHANNELS`
    pub fn channels_with_frames<const NUM_CHANNELS: usize>(
        &self,
        frames: usize,
    ) -> [&[T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS and `frames` above, so this is always
        // within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.num_frames),
                    frames,
                )
            })
        }
    }

    /// Get a mutable reference to the first given number of channels with the given
    /// number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    ///
    /// # Panics
    /// Panics if `NUM_CHANNELS > Self::CHANNELS`
    pub fn channels_with_frames_mut<const NUM_CHANNELS: usize>(
        &mut self,
        frames: usize,
    ) -> [&mut [T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS and `frames` above, so this is always
        // within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.num_frames),
                    frames,
                )
            })
        }
    }

    /// Get an immutable reference to all channels in this buffer.
    pub fn all(&self) -> [&[T]; CHANNELS] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.num_frames),
                    self.num_frames,
                )
            })
        }
    }

    /// Get a mutable reference to all channels in this buffer.
    pub fn all_mut(&mut self) -> [&mut [T]; CHANNELS] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.num_frames),
                    self.num_frames,
                )
            })
        }
    }

    /// Get an immutable reference to all channels with the given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn all_with_frames(&self, frames: usize) -> [&[T]; CHANNELS] {
        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.num_frames),
                    frames,
                )
            })
        }
    }

    /// Get a mutable reference to all channels with the given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn all_with_frames_mut(&mut self, frames: usize) -> [&mut [T]; CHANNELS] {
        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.num_frames),
                    frames,
                )
            })
        }
    }
}

impl<T: Clone + Copy + Default, const CHANNELS: usize> Clone
    for ConstSequentialBuffer<T, CHANNELS>
{
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.num_frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}

/// A buffer of samples where each channel is stored sequentially in memory.
///
/// `T` is the backing type of the storage, typically f32.
///
/// The number of frames and number of channels cannot be changed once constructed.
#[derive(Debug)]
pub struct SequentialBuffer<T: Clone + Copy + Default> {
    buffer: Vec<T>,
    num_channels: NonZeroUsize,
    num_frames: usize,
}

impl<T: Clone + Copy + Default> SequentialBuffer<T> {
    pub fn new(channels: NonZeroUsize, frames: usize) -> Self {
        let buffer_len = frames * channels.get();

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self {
            buffer,
            num_channels: channels,
            num_frames: frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.num_frames
    }

    pub fn num_channels(&self) -> NonZeroUsize {
        self.num_channels
    }

    /// Returns an immutable slice of the given channel. This slice will have a length of
    /// `self.frames()`.
    ///
    /// Returns `None` if `channel >= self.num_channels()`.
    pub fn channel_slice(&self, channel: usize) -> Option<&[T]> {
        if channel < self.num_channels.get() {
            // SAFETY:
            //
            // * The constructor has set the size of the buffer to `self.frames * self.channels`,
            // and we have checked the size of `channels` above, so this is always within range.
            unsafe {
                Some(core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(channel * self.num_frames),
                    self.num_frames,
                ))
            }
        } else {
            None
        }
    }

    /// Returns a mutable slice to the given channel. This slice will have a length of
    /// `self.frames()`.
    ///
    /// Returns `None` if `channel >= self.num_channels()`.
    pub fn channel_slice_mut(&mut self, channel: usize) -> Option<&mut [T]> {
        if channel < self.num_channels.get() {
            // SAFETY:
            //
            // * The constructor has set the size of the buffer to `self.frames * self.channels`,
            // and we have checked the size of `channels` above, so this is always within range.
            // * None of these slices overlap, and `self` is borrowed mutably in this method,
            // so all mutability rules are being upheld.
            unsafe {
                Some(core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(channel * self.num_frames),
                    self.num_frames,
                ))
            }
        } else {
            None
        }
    }

    /// Returns two immutable slices to the first two channels in this buffer with length
    /// `frames`.
    ///
    /// Clamps `frames` to the available data.
    ///
    /// If the number of channels in this buffer is less than 2, then this will return
    /// `None`.
    pub fn stereo(&self, frames: usize) -> Option<(&[T], &[T])> {
        if self.num_channels.get() < 2 {
            return None;
        }

        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // we checked there is at least two channels above, and we have constrained `frames`
        // above, so this is always within range.
        unsafe {
            Some((
                core::slice::from_raw_parts(self.buffer.as_ptr(), frames),
                core::slice::from_raw_parts(self.buffer.as_ptr().add(self.num_frames), frames),
            ))
        }
    }

    /// Returns two mutable slices to the first two channels in this buffer with length
    /// `frames`.
    ///
    /// Clamps `frames` to the available data.
    ///
    /// If the number of channels in this buffer is less than 2, then this will return
    /// `None`.
    pub fn stereo_mut(&mut self, frames: usize) -> Option<(&mut [T], &mut [T])> {
        if self.num_channels.get() < 2 {
            return None;
        }

        let frames = frames.min(self.num_frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // we checked there is at least two channels above, and we have constrained `frames`
        // above, so this is always within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            Some((
                core::slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), frames),
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(self.num_frames),
                    frames,
                ),
            ))
        }
    }

    /// Returns a vec of slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present.
    ///
    /// Clamps `num_channels` and `frames` to the available data.
    pub fn channels<const MAX_CHANNELS: usize>(
        &self,
        num_channels: usize,
        frames: usize,
    ) -> ArrayVec<&[T], MAX_CHANNELS> {
        let frames = frames.min(self.num_frames);
        let channels = num_channels.min(self.num_channels.get()).min(MAX_CHANNELS);

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // and we have constrained the size of `channels` and `frames` above, so this is always
        // within range.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.num_frames),
                    frames,
                ));
            }
        }

        res
    }

    /// Returns a vec of mutable slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present.
    ///
    /// Clamps `num_channels` and `frames` to the available data.
    pub fn channels_mut<const MAX_CHANNELS: usize>(
        &mut self,
        num_channels: usize,
        frames: usize,
    ) -> ArrayVec<&mut [T], MAX_CHANNELS> {
        let frames = frames.min(self.num_frames);
        let channels = num_channels.min(self.num_channels.get()).min(MAX_CHANNELS);

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // and we have constrained `channels` and `frames` above, so this is always
        // within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.num_frames),
                    frames,
                ));
            }
        }

        res
    }
}

impl<T: Clone + Copy + Default> Clone for SequentialBuffer<T> {
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.num_channels, self.num_frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}

/// A memory-efficient buffer of samples with a given number of instances each with a given
/// number of channels. Each channel has a length of `frames`.
///
/// `T` is the backing type of the storage, typically f32.
///
/// This is like a collection of `num_instances` [`SequentialBuffer`]s but contiguous in memory.
///
/// The number of instances, channels, and frames cannot be changed once constructed.
#[derive(Debug)]
pub struct InstanceBuffer<T: Clone + Copy + Default> {
    buffer: Vec<T>,
    num_instances: usize,
    num_channels: NonZeroUsize,
    num_frames: usize,
}

impl<T: Clone + Copy + Default> InstanceBuffer<T> {
    pub fn new(instances: usize, channels: NonZeroUsize, frames: usize) -> Self {
        let buffer_len = frames * channels.get() * instances;

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self {
            buffer,
            num_instances: instances,
            num_channels: channels,
            num_frames: frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.num_frames
    }

    pub fn num_channels(&self) -> NonZeroUsize {
        self.num_channels
    }

    pub fn num_instances(&self) -> usize {
        self.num_instances
    }

    /// Returns an array of slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present of the instance at `instance_index`.
    ///
    /// Clamps `num_channels` and `frames` to the available data.
    ///
    /// Returns `None` if there is no instance at `instance_index`.
    pub fn instance<const MAX_CHANNELS: usize>(
        &self,
        instance_index: usize,
        channels: usize,
        frames: usize,
    ) -> Option<ArrayVec<&[T], MAX_CHANNELS>> {
        if instance_index >= self.num_instances {
            return None;
        }

        let frames = frames.min(self.num_frames);
        let channels = channels.min(self.num_channels.get()).min(MAX_CHANNELS);

        let start_frame = instance_index * self.num_frames * self.num_channels.get();

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to
        // `self.frames * self.channels * self.num_instances`, and we have constrained
        // `instance_index`, `channels` and `frames` above, so this is always within range.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts(
                    self.buffer
                        .as_ptr()
                        .add(start_frame + (ch_i * self.num_frames)),
                    frames,
                ));
            }
        }

        Some(res)
    }

    /// Returns an array of mutable slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present of the instance at `instance_index`.
    ///
    /// Clamps `num_channels` and `frames` to the available data.
    ///
    /// Returns `None` if there is no instance at `instance_index`.
    pub fn instance_mut<const MAX_CHANNELS: usize>(
        &mut self,
        instance_index: usize,
        channels: usize,
        frames: usize,
    ) -> Option<ArrayVec<&mut [T], MAX_CHANNELS>> {
        if instance_index >= self.num_instances {
            return None;
        }

        let frames = frames.min(self.num_frames);
        let channels = channels.min(self.num_channels.get()).min(MAX_CHANNELS);

        let start_frame = instance_index * self.num_frames * self.num_channels.get();

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to
        // `self.frames * self.channels * self.num_instances`, and we have constrained
        // `instance_index`, `channels` and `frames` above, so this is always within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts_mut(
                    self.buffer
                        .as_mut_ptr()
                        .add(start_frame + (ch_i * self.num_frames)),
                    frames,
                ));
            }
        }

        Some(res)
    }
}

impl<T: Clone + Copy + Default> Clone for InstanceBuffer<T> {
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.num_instances, self.num_channels, self.num_frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}
