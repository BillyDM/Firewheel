use crate::SilenceMask;

/// De-interleave audio channels
pub fn deinterleave<V: AsMut<[f32]>>(
    channels: &mut [V],
    interleaved: &[f32],
    num_interleaved_channels: usize,
    calculate_silence_mask: bool,
) -> SilenceMask {
    if channels.is_empty() {
        return SilenceMask::NONE_SILENT;
    }

    if num_interleaved_channels == 0 {
        for ch in channels.iter_mut() {
            ch.as_mut().fill(0.0);
        }

        return SilenceMask::new_all_silent(channels.len());
    }

    let mut silence_mask = SilenceMask::NONE_SILENT;

    let (num_filled_channels, samples) = if num_interleaved_channels == 1 {
        // Mono, no need to deinterleave.

        let samples = interleaved.len();
        let ch = &mut channels[0].as_mut()[..samples];

        ch.copy_from_slice(interleaved);

        if calculate_silence_mask {
            if ch.iter().find(|&&s| s != 0.0).is_none() {
                silence_mask.set_channel(0, true);
            }
        }

        (1, samples)
    } else if num_interleaved_channels == 2 && channels.len() >= 2 {
        // Provide an optimized loop for stereo.

        let samples = interleaved.len() / 2;

        let (ch0, ch1) = channels.split_first_mut().unwrap();
        let ch0 = &mut ch0.as_mut()[..samples];
        let ch1 = &mut ch1[0].as_mut()[..samples];

        for (in_chunk, (ch0_s, ch1_s)) in interleaved
            .chunks_exact(2)
            .zip(ch0.iter_mut().zip(ch1.iter_mut()))
        {
            *ch0_s = in_chunk[0];
            *ch1_s = in_chunk[1];
        }

        if calculate_silence_mask {
            for (ch_i, ch) in channels.iter_mut().enumerate() {
                if ch.as_mut()[0..samples]
                    .iter()
                    .find(|&&s| s != 0.0)
                    .is_none()
                {
                    silence_mask.set_channel(ch_i, true);
                }
            }
        }

        (2, samples)
    } else {
        let mut num_filled_channels = 0;
        let samples = interleaved.len() / num_interleaved_channels;

        for (ch_i, ch) in (0..num_interleaved_channels).zip(channels.iter_mut()) {
            let ch = &mut ch.as_mut()[..samples];

            for (in_chunk, out_s) in interleaved
                .chunks_exact(num_interleaved_channels)
                .zip(ch.iter_mut())
            {
                *out_s = in_chunk[ch_i];
            }

            if calculate_silence_mask && ch_i < 64 {
                if ch.iter().find(|&&s| s != 0.0).is_none() {
                    silence_mask.set_channel(ch_i, true);
                }
            }

            num_filled_channels += 1;
        }

        (num_filled_channels, samples)
    };

    if num_filled_channels < channels.len() {
        for (ch_i, ch) in channels.iter_mut().enumerate().skip(num_filled_channels) {
            ch.as_mut()[..samples].fill(0.0);

            if calculate_silence_mask && ch_i < 64 {
                silence_mask.set_channel(ch_i, true);
            }
        }
    }

    silence_mask
}

/// Interleave audio channels
pub fn interleave<V: AsRef<[f32]>>(
    channels: &[V],
    interleaved: &mut [f32],
    num_interleaved_channels: usize,
    silence_mask: Option<SilenceMask>,
) {
    if channels.is_empty() || num_interleaved_channels == 0 {
        interleaved.fill(0.0);
        return;
    }

    if let Some(silence_mask) = silence_mask {
        if channels.len() <= 64 {
            if silence_mask.all_channels_silent(channels.len()) {
                interleaved.fill(0.0);
                return;
            }
        }
    }

    if num_interleaved_channels == 1 {
        // Mono, no need to interleave.
        interleaved.copy_from_slice(&channels[0].as_ref()[..interleaved.len()]);
        return;
    }

    if num_interleaved_channels == 2 && channels.len() >= 2 {
        // Provide an optimized loop for stereo.
        let samples = interleaved.len() / 2;

        let ch1 = &channels[0].as_ref()[..samples];
        let ch2 = &channels[1].as_ref()[..samples];

        for (out_chunk, (&ch1_s, &ch2_s)) in interleaved
            .chunks_exact_mut(2)
            .zip(ch1.iter().zip(ch2.iter()))
        {
            out_chunk[0] = ch1_s;
            out_chunk[1] = ch2_s;
        }

        return;
    }

    let any_channel_silent = if let Some(silence_mask) = silence_mask {
        if channels.len() <= 64 {
            silence_mask.any_channel_silent(channels.len())
        } else {
            true
        }
    } else {
        false
    };

    if num_interleaved_channels > channels.len() || any_channel_silent {
        interleaved.fill(0.0);
    }

    for (ch_i, ch) in channels.iter().enumerate() {
        if let Some(silence_mask) = silence_mask {
            if ch_i < 64 {
                if silence_mask.is_channel_silent(ch_i) {
                    continue;
                }
            }
        }

        for (out_chunk, &in_s) in interleaved
            .chunks_exact_mut(num_interleaved_channels)
            .zip(ch.as_ref().iter())
        {
            out_chunk[ch_i] = in_s;
        }
    }
}
