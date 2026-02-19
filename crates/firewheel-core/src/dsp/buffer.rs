use core::num::NonZeroUsize;

use arrayvec::ArrayVec;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

/// A memory-efficient buffer of samples with `CHANNELS` channels. Each channel
/// has a length of `frames`. `T` is the backing type of the storage, typically f32.
/// This is like a [`VarChannelBuffer`] but guarantees all `MAX_CHANNELS` are present.
#[derive(Debug)]
pub struct ChannelBuffer<T: Clone + Copy + Default, const CHANNELS: usize> {
    buffer: Vec<T>,
    frames: usize,
}

impl<T: Clone + Copy + Default, const CHANNELS: usize> ChannelBuffer<T, CHANNELS> {
    pub const fn empty() -> Self {
        assert!(CHANNELS > 0);

        Self {
            buffer: Vec::new(),
            frames: 0,
        }
    }

    pub fn new(frames: usize) -> Self {
        assert!(CHANNELS > 0);

        let buffer_len = frames * CHANNELS;

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self { buffer, frames }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Get an immutable reference to the first channel.
    #[inline]
    pub fn first(&self) -> &[T] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        unsafe { core::slice::from_raw_parts(self.buffer.as_ptr(), self.frames) }
    }

    /// Get a mutable reference to the first channel.
    #[inline]
    pub fn first_mut(&mut self) -> &mut [T] {
        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`.
        // * `self` is borrowed mutably in this method, so all mutability rules are
        // being upheld.
        unsafe { core::slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), self.frames) }
    }

    /// Get an immutable reference to the first channel with the given number of
    /// frames.
    ///
    /// The length of the returned slice will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    #[inline]
    pub fn first_with_frames(&self, frames: usize) -> &[T] {
        let frames = frames.min(self.frames);

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
        let frames = frames.min(self.frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        // * `self` is borrowed mutably in this method, so all mutability rules are
        // being upheld.
        unsafe { core::slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), frames) }
    }

    /// Get an immutable reference to the first given number of channels in this buffer.
    pub fn channels<const NUM_CHANNELS: usize>(&self) -> [&[T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS above, so this is always within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.frames),
                    self.frames,
                )
            })
        }
    }

    /// Get a mutable reference to the first given number of channels in this buffer.
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
                    self.buffer.as_mut_ptr().add(ch_i * self.frames),
                    self.frames,
                )
            })
        }
    }

    /// Get an immutable reference to the first given number of channels with the
    /// given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn channels_with_frames<const NUM_CHANNELS: usize>(
        &self,
        frames: usize,
    ) -> [&[T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        let frames = frames.min(self.frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained NUM_CHANNELS and `frames` above, so this is always
        // within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(self.buffer.as_ptr().add(ch_i * self.frames), frames)
            })
        }
    }

    /// Get a mutable reference to the first given number of channels with the given
    /// number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn channels_with_frames_mut<const NUM_CHANNELS: usize>(
        &mut self,
        frames: usize,
    ) -> [&mut [T]; NUM_CHANNELS] {
        assert!(NUM_CHANNELS <= CHANNELS);

        let frames = frames.min(self.frames);

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
                    self.buffer.as_mut_ptr().add(ch_i * self.frames),
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
                    self.buffer.as_ptr().add(ch_i * self.frames),
                    self.frames,
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
                    self.buffer.as_mut_ptr().add(ch_i * self.frames),
                    self.frames,
                )
            })
        }
    }

    /// Get an immutable reference to all channels with the given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn all_with_frames(&self, frames: usize) -> [&[T]; CHANNELS] {
        let frames = frames.min(self.frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts(self.buffer.as_ptr().add(ch_i * self.frames), frames)
            })
        }
    }

    /// Get a mutable reference to all channels with the given number of frames.
    ///
    /// The length of the returned slices will be either `frames` or the number of
    /// frames in this buffer, whichever is smaller.
    pub fn all_with_frames_mut(&mut self, frames: usize) -> [&mut [T]; CHANNELS] {
        let frames = frames.min(self.frames);

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * CHANNELS`,
        // and we have constrained `frames` above, so this is always within range.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            core::array::from_fn(|ch_i| {
                core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.frames),
                    frames,
                )
            })
        }
    }
}

impl<T: Clone + Copy + Default, const CHANNELS: usize> Clone for ChannelBuffer<T, CHANNELS> {
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}

/// A memory-efficient buffer of samples with up to `MAX_CHANNELS` channels. Each
/// channel has a length of `frames`. `T` is the backing type of the storage, typically f32.
/// This is like a [`ChannelBuffer`] but does not guarantee all `MAX_CHANNELS` are present.
/// There is only at least one channel, and the number of channels is immutable once constructed.
#[derive(Debug)]
pub struct VarChannelBuffer<T: Clone + Copy + Default, const MAX_CHANNELS: usize> {
    buffer: Vec<T>,
    channels: NonZeroUsize,
    frames: usize,
}

impl<T: Clone + Copy + Default, const MAX_CHANNELS: usize> VarChannelBuffer<T, MAX_CHANNELS> {
    pub fn new(channels: NonZeroUsize, frames: usize) -> Self {
        assert!(channels.get() <= MAX_CHANNELS);

        let buffer_len = frames * channels.get();

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self {
            buffer,
            channels,
            frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    /// Returns a vec of slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present.
    /// Clamps `num_channels` and `frames` to the available data.
    pub fn channels(&self, num_channels: usize, frames: usize) -> ArrayVec<&[T], MAX_CHANNELS> {
        let frames = frames.min(self.frames);
        let channels = num_channels.min(self.channels.get());

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // and we have constrained `channels` and `frames` above, so this is always
        // within range.
        // * The constructor has ensured that `self.channels <= MAX_CHANNELS`.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(ch_i * self.frames),
                    frames,
                ));
            }
        }

        res
    }

    /// Returns a vec of mutable slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present.
    /// Clamps `num_channels` and `frames` to the available data.
    pub fn channels_mut(
        &mut self,
        num_channels: usize,
        frames: usize,
    ) -> ArrayVec<&mut [T], MAX_CHANNELS> {
        let frames = frames.min(self.frames);
        let channels = num_channels.min(self.channels.get());

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to `self.frames * self.channels`,
        // and we have constrained `channels` and `frames` above, so this is always
        // within range.
        // * The constructor has ensured that `self.channels <= MAX_CHANNELS`.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts_mut(
                    self.buffer.as_mut_ptr().add(ch_i * self.frames),
                    frames,
                ));
            }
        }

        res
    }
}

impl<T: Clone + Copy + Default, const MAX_CHANNELS: usize> Clone
    for VarChannelBuffer<T, MAX_CHANNELS>
{
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.channels, self.frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}

/// A memory-efficient buffer of samples with variable number of instances each with up to
/// `MAX_CHANNELS` channels. Each channel has a length of `frames`.
/// `T` is the backing type of the storage, typically f32.
/// This is like a collection of `num_instances` [`VarChannelBuffer`]s but contiguous in memory.
/// Instances may not be added or removed once constructed.
#[derive(Debug)]
pub struct InstanceBuffer<T: Clone + Copy + Default, const MAX_CHANNELS: usize> {
    buffer: Vec<T>,
    num_instances: usize,
    channels: NonZeroUsize,
    frames: usize,
}

impl<T: Clone + Copy + Default, const MAX_CHANNELS: usize> InstanceBuffer<T, MAX_CHANNELS> {
    pub fn new(num_instances: usize, channels: NonZeroUsize, frames: usize) -> Self {
        assert!(channels.get() <= MAX_CHANNELS);

        let buffer_len = frames * channels.get() * num_instances;

        let mut buffer = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, Default::default());

        Self {
            buffer,
            num_instances,
            channels,
            frames,
        }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn num_channels(&self) -> NonZeroUsize {
        self.channels
    }

    pub fn num_instances(&self) -> usize {
        self.num_instances
    }

    /// Returns a vec of slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present of the instance at `instance_index`.
    /// Clamps `num_channels` and `frames` to the available data.
    /// Returns None if there is no instance at `instance_index`.
    pub fn instance(
        &self,
        instance_index: usize,
        channels: usize,
        frames: usize,
    ) -> Option<ArrayVec<&[T], MAX_CHANNELS>> {
        if instance_index >= self.num_instances {
            return None;
        }

        let frames = frames.min(self.frames);
        let channels = channels.min(self.channels.get());

        let start_frame = instance_index * self.frames * self.channels.get();

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to
        // `self.frames * self.channels * self.num_instances`, and we have constrained
        // `instance_index`, `channels` and `frames` above, so this is always within range.
        // * The constructor has ensured that `self.channels <= MAX_CHANNELS`.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts(
                    self.buffer.as_ptr().add(start_frame + (ch_i * self.frames)),
                    frames,
                ));
            }
        }

        Some(res)
    }

    /// Returns a vec of mutable slices to the first `frames` frames of the backing buffers
    /// for the first `num_channels` channels present of the instance at `instance_index`.
    /// Clamps `num_channels` and `frames` to the available data.
    /// Returns None if there is no instance at `instance_index`.
    pub fn instance_mut(
        &mut self,
        instance_index: usize,
        channels: usize,
        frames: usize,
    ) -> Option<ArrayVec<&mut [T], MAX_CHANNELS>> {
        if instance_index >= self.num_instances {
            return None;
        }

        let frames = frames.min(self.frames);
        let channels = channels.min(self.channels.get());

        let start_frame = instance_index * self.frames * self.channels.get();

        let mut res = ArrayVec::new();

        // SAFETY:
        //
        // * The constructor has set the size of the buffer to
        // `self.frames * self.channels * self.num_instances`, and we have constrained
        // `instance_index`, `channels` and `frames` above, so this is always within range.
        // * The constructor has ensured that `self.channels <= MAX_CHANNELS`.
        // * None of these slices overlap, and `self` is borrowed mutably in this method,
        // so all mutability rules are being upheld.
        unsafe {
            for ch_i in 0..channels {
                res.push_unchecked(core::slice::from_raw_parts_mut(
                    self.buffer
                        .as_mut_ptr()
                        .add(start_frame + (ch_i * self.frames)),
                    frames,
                ));
            }
        }

        Some(res)
    }
}

impl<T: Clone + Copy + Default, const MAX_CHANNELS: usize> Clone
    for InstanceBuffer<T, MAX_CHANNELS>
{
    fn clone(&self) -> Self {
        // Ensure that `reserve_exact` is used when cloning.
        let mut new_self = Self::new(self.num_instances, self.channels, self.frames);
        new_self.buffer.copy_from_slice(&self.buffer);
        new_self
    }
}
