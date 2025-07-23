use core::num::NonZeroU32;

use firewheel_core::{
    clock::{
        DurationSamples, InstantMusical, InstantSamples, MusicalTransport, ProcTransportInfo,
        TransportState,
    },
    node::TransportInfo,
};

pub(super) struct ProcTransportState {
    transport_state: Box<TransportState>,
    start_clock_samples: InstantSamples,
    paused_at_clock_samples: InstantSamples,
    paused_at_musical_time: InstantMusical,
}

impl ProcTransportState {
    pub fn new() -> Self {
        Self {
            transport_state: Box::new(TransportState::default()),
            start_clock_samples: InstantSamples(0),
            paused_at_clock_samples: InstantSamples(0),
            paused_at_musical_time: InstantMusical(0.0),
        }
    }

    pub fn musical_to_samples(
        &self,
        musical: InstantMusical,
        sample_rate: NonZeroU32,
    ) -> Option<InstantSamples> {
        self.transport_state
            .transport
            .as_ref()
            .filter(|_| *self.transport_state.playing)
            .map(|transport| {
                transport.musical_to_samples(musical, self.start_clock_samples, sample_rate)
            })
    }

    /// Returns the old transport state
    pub fn set_transport_state(
        &mut self,
        mut new_transport_state: Box<TransportState>,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> Box<TransportState> {
        let mut did_pause = false;

        if let Some(new_transport) = &new_transport_state.transport {
            if self.transport_state.playhead != new_transport_state.playhead
                || self.transport_state.transport.is_none()
            {
                self.start_clock_samples = new_transport.transport_start(
                    clock_samples,
                    *new_transport_state.playhead,
                    sample_rate,
                );
            } else {
                let old_transport = self.transport_state.transport.as_ref().unwrap();

                if *new_transport_state.playing {
                    if !*self.transport_state.playing {
                        // Resume
                        if old_transport == new_transport {
                            self.start_clock_samples +=
                                clock_samples - self.paused_at_clock_samples;
                        } else {
                            self.start_clock_samples = new_transport.transport_start(
                                clock_samples,
                                self.paused_at_musical_time,
                                sample_rate,
                            );
                        }
                    } else if old_transport != new_transport {
                        // Continue where the previous left off
                        let current_playhead = old_transport.samples_to_musical(
                            clock_samples,
                            self.start_clock_samples,
                            sample_rate,
                            sample_rate_recip,
                        );
                        self.start_clock_samples = new_transport.transport_start(
                            clock_samples,
                            current_playhead,
                            sample_rate,
                        );
                    }
                } else if *self.transport_state.playing {
                    // Pause
                    did_pause = true;

                    self.paused_at_clock_samples = clock_samples;
                    self.paused_at_musical_time = old_transport.samples_to_musical(
                        clock_samples,
                        self.start_clock_samples,
                        sample_rate,
                        sample_rate_recip,
                    );
                }
            }
        }

        if !did_pause {
            self.paused_at_clock_samples = clock_samples;
            self.paused_at_musical_time = *new_transport_state.playhead;
        }

        core::mem::swap(&mut new_transport_state, &mut self.transport_state);
        new_transport_state
    }

    pub fn process_block(
        &mut self,
        frames: usize,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> ProcTransportInfo {
        let Some(transport) = &self.transport_state.transport else {
            return ProcTransportInfo {
                frames,
                beats_per_minute: 0.0,
                delta_beats_per_minute: 0.0,
            };
        };

        let mut playhead = transport.samples_to_musical(
            clock_samples,
            self.start_clock_samples,
            sample_rate,
            sample_rate_recip,
        );
        let beats_per_minute = transport.bpm_at_musical(playhead);

        if !*self.transport_state.playing {
            return ProcTransportInfo {
                frames,
                beats_per_minute,
                delta_beats_per_minute: 0.0,
            };
        }

        let mut loop_end_clock_samples = InstantSamples::default();
        let mut stop_at_clock_samples = InstantSamples::default();

        if let Some(loop_range) = &self.transport_state.loop_range {
            loop_end_clock_samples =
                transport.musical_to_samples(loop_range.end, self.start_clock_samples, sample_rate);

            if clock_samples >= loop_end_clock_samples {
                // Loop back to start of loop.
                self.start_clock_samples =
                    transport.transport_start(clock_samples, loop_range.start, sample_rate);
                playhead = loop_range.start;
            }
        } else if let Some(stop_at) = self.transport_state.stop_at {
            stop_at_clock_samples =
                transport.musical_to_samples(stop_at, self.start_clock_samples, sample_rate);

            if clock_samples >= stop_at_clock_samples {
                // Stop the transport.
                *self.transport_state.playing = false;
                return ProcTransportInfo {
                    frames,
                    beats_per_minute,
                    delta_beats_per_minute: 0.0,
                };
            }
        }

        let mut info = transport.proc_transport_info(frames, playhead);

        let proc_end_samples = clock_samples + DurationSamples(info.frames as i64);

        if self.transport_state.loop_range.is_some() {
            if proc_end_samples > loop_end_clock_samples {
                // End of the loop reached.
                info.frames = (loop_end_clock_samples - clock_samples).0.max(0) as usize;
            }
        } else if self.transport_state.stop_at.is_some() {
            if proc_end_samples > stop_at_clock_samples {
                // End of the transport reached.
                info.frames = (stop_at_clock_samples - clock_samples).0.max(0) as usize;
            }
        }

        info
    }

    pub fn transport_info(
        &mut self,
        proc_transport_info: &ProcTransportInfo,
    ) -> Option<TransportInfo> {
        self.transport_state
            .transport
            .as_ref()
            .map(|transport| TransportInfo {
                transport,
                start_clock_samples: self
                    .transport_state
                    .playing
                    .then(|| self.start_clock_samples),
                beats_per_minute: proc_transport_info.beats_per_minute,
                delta_bpm_per_frame: proc_transport_info.delta_beats_per_minute,
            })
    }

    /// Returns (current_playhead, transport_is_playing)
    pub fn shared_clock_info(
        &self,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> (Option<InstantMusical>, bool) {
        self.transport_state
            .transport
            .as_ref()
            .map(|transport| {
                if *self.transport_state.playing {
                    let current_playhead = transport.samples_to_musical(
                        clock_samples,
                        self.start_clock_samples,
                        sample_rate,
                        sample_rate_recip,
                    );

                    (Some(current_playhead), true)
                } else {
                    (Some(self.paused_at_musical_time), false)
                }
            })
            .unwrap_or((None, false))
    }

    /// Returns `Option<transport, start_clock_samples>`.
    pub fn transport_and_start_clock_samples(&self) -> Option<(&MusicalTransport, InstantSamples)> {
        self.transport_state
            .transport
            .as_ref()
            .map(|transport| (transport, self.start_clock_samples))
    }

    pub fn update_sample_rate(
        &mut self,
        old_sample_rate: NonZeroU32,
        old_sample_rate_recip: f64,
        new_sample_rate: NonZeroU32,
    ) {
        self.start_clock_samples = self
            .start_clock_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
        self.paused_at_clock_samples = self
            .paused_at_clock_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
    }
}
