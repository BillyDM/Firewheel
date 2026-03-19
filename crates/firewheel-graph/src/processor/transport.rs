use core::num::NonZeroU32;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Box;

use firewheel_core::{
    clock::{
        DurationSamples, EventInstant, InstantMusical, InstantSamples, MusicalTransport,
        ProcTransportInfo, TransportSpeed, TransportState,
    },
    node::TransportInfo,
};

#[derive(Clone, Copy)]
struct AutomationState {
    keyframe_index: usize,
    move_to_next_keyframe: bool,
}

pub(super) struct ProcTransportState {
    transport_state: Box<TransportState>,
    transport_start_samples: InstantSamples,
    paused_at_clock_samples: InstantSamples,
    paused_at_musical_time: InstantMusical,
    current_speed_multiplier: f64,
    automation_state: Option<AutomationState>,
}

impl ProcTransportState {
    pub fn new() -> Self {
        Self {
            transport_state: Box::new(TransportState::default()),
            transport_start_samples: InstantSamples(0),
            paused_at_clock_samples: InstantSamples(0),
            paused_at_musical_time: InstantMusical(0.0),
            current_speed_multiplier: 1.0,
            automation_state: None,
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
                transport.musical_to_samples(
                    musical,
                    self.transport_start_samples,
                    self.current_speed_multiplier,
                    sample_rate,
                )
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

        self.automation_state = None;
        match &new_transport_state.speed {
            TransportSpeed::Static {
                multiplier,
                start_at: change_at,
            } => {
                if change_at.is_none() {
                    self.current_speed_multiplier = *multiplier;
                }
            }
            TransportSpeed::Automate {
                keyframes,
                start_at: start_instant,
                ..
            } => {
                if start_instant.is_none() {
                    self.current_speed_multiplier = keyframes[0].multiplier;

                    if keyframes.len() > 1 {
                        self.automation_state = Some(AutomationState {
                            keyframe_index: 0,
                            move_to_next_keyframe: false,
                        });
                    }
                }
            }
        }

        assert!(self.current_speed_multiplier.is_finite() && self.current_speed_multiplier > 0.0);

        if let Some(new_transport) = &new_transport_state.transport {
            if self.transport_state.playhead != new_transport_state.playhead
                || self.transport_state.transport.is_none()
            {
                self.transport_start_samples = new_transport.transport_start(
                    clock_samples,
                    *new_transport_state.playhead,
                    self.current_speed_multiplier,
                    sample_rate,
                );
            } else {
                let old_transport = self.transport_state.transport.as_ref().unwrap();

                if *new_transport_state.playing {
                    if !*self.transport_state.playing {
                        // Resume
                        if old_transport == new_transport {
                            self.transport_start_samples +=
                                clock_samples - self.paused_at_clock_samples;
                        } else {
                            self.transport_start_samples = new_transport.transport_start(
                                clock_samples,
                                self.paused_at_musical_time,
                                self.current_speed_multiplier,
                                sample_rate,
                            );
                        }
                    } else if old_transport != new_transport {
                        // Continue where the previous left off
                        let current_playhead = old_transport.samples_to_musical(
                            clock_samples,
                            self.transport_start_samples,
                            self.current_speed_multiplier,
                            sample_rate,
                            sample_rate_recip,
                        );
                        self.transport_start_samples = new_transport.transport_start(
                            clock_samples,
                            current_playhead,
                            self.current_speed_multiplier,
                            sample_rate,
                        );
                    }
                } else if *self.transport_state.playing {
                    // Pause
                    did_pause = true;

                    self.paused_at_clock_samples = clock_samples;
                    self.paused_at_musical_time = old_transport.samples_to_musical(
                        clock_samples,
                        self.transport_start_samples,
                        self.current_speed_multiplier,
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
        mut frames: usize,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> ProcTransportInfo {
        let Some(transport) = &self.transport_state.transport else {
            return ProcTransportInfo {
                frames,
                beats_per_minute: 0.0,
            };
        };

        match &mut self.transport_state.speed {
            TransportSpeed::Static {
                multiplier,
                start_at: change_at,
            } => {
                if let Some(change_at_musical) = *change_at {
                    let change_at_samples = transport.musical_to_samples(
                        change_at_musical,
                        self.transport_start_samples,
                        self.current_speed_multiplier,
                        sample_rate,
                    );

                    if clock_samples >= change_at_samples {
                        self.current_speed_multiplier = *multiplier;
                        *change_at = None;
                    } else if ((change_at_samples.0 - clock_samples.0) as usize) < frames {
                        frames = (change_at_samples.0 - clock_samples.0) as usize;
                    }
                }
            }
            TransportSpeed::Automate {
                keyframes,
                start_at: start_instant,
            } => {
                let mut remove_automation_state = false;
                if let Some(automation_state) = &mut self.automation_state {
                    if automation_state.move_to_next_keyframe {
                        automation_state.move_to_next_keyframe = false;
                        automation_state.keyframe_index += 1;

                        if let Some(keyframe) = keyframes.get(automation_state.keyframe_index) {
                            self.current_speed_multiplier = keyframe.multiplier;

                            if automation_state.keyframe_index == keyframes.len() - 1 {
                                remove_automation_state = true;
                            }
                        } else {
                            remove_automation_state = true;
                        }
                    }
                }

                if remove_automation_state {
                    self.automation_state = None;
                }

                if let Some(automation_state) = &mut self.automation_state {
                    if let Some(next_keyframe) = keyframes.get(automation_state.keyframe_index + 1)
                    {
                        let keyframe_start_samples = match next_keyframe.instant {
                            EventInstant::AtClockSeconds(seconds) => {
                                seconds.to_samples(sample_rate)
                            }
                            EventInstant::AtClockSamples(samples) => samples,
                            EventInstant::DelaySeconds(seconds) => {
                                clock_samples + seconds.to_samples(sample_rate)
                            }
                            EventInstant::DelaySamples(samples) => clock_samples + samples,
                            EventInstant::AtClockMusical(musical) => transport.musical_to_samples(
                                musical,
                                self.transport_start_samples,
                                self.current_speed_multiplier,
                                sample_rate,
                            ),
                        };

                        if clock_samples + DurationSamples(frames as i64) > keyframe_start_samples {
                            frames = (keyframe_start_samples.0 - clock_samples.0) as usize;
                            automation_state.move_to_next_keyframe = true;
                        }
                    }
                }

                if let Some(start_instant_musical) = *start_instant {
                    let start_instant_samples = transport.musical_to_samples(
                        start_instant_musical,
                        self.transport_start_samples,
                        self.current_speed_multiplier,
                        sample_rate,
                    );

                    if clock_samples >= start_instant_samples {
                        self.current_speed_multiplier = keyframes[0].multiplier;
                        *start_instant = None;

                        if keyframes.len() > 1 {
                            self.automation_state = Some(AutomationState {
                                keyframe_index: 0,
                                move_to_next_keyframe: false,
                            });
                        } else {
                            self.automation_state = None;
                        }
                    } else if ((start_instant_samples.0 - clock_samples.0) as usize) < frames {
                        frames = (start_instant_samples.0 - clock_samples.0) as usize;
                    }
                }
            }
        }

        assert!(self.current_speed_multiplier.is_finite() && self.current_speed_multiplier > 0.0);

        self.process_block_inner(frames, clock_samples, sample_rate, sample_rate_recip)
    }

    fn process_block_inner(
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
            };
        };

        let mut playhead = transport.samples_to_musical(
            clock_samples,
            self.transport_start_samples,
            self.current_speed_multiplier,
            sample_rate,
            sample_rate_recip,
        );
        let beats_per_minute = transport.bpm_at_musical(playhead, self.current_speed_multiplier);

        if !*self.transport_state.playing {
            return ProcTransportInfo {
                frames,
                beats_per_minute,
            };
        }

        let mut loop_end_clock_samples = InstantSamples::default();
        let mut stop_at_clock_samples = InstantSamples::default();

        if let Some(loop_range) = &self.transport_state.loop_range {
            loop_end_clock_samples = transport.musical_to_samples(
                loop_range.end,
                self.transport_start_samples,
                self.current_speed_multiplier,
                sample_rate,
            );

            if clock_samples >= loop_end_clock_samples {
                // Loop back to start of loop.
                self.transport_start_samples = transport.transport_start(
                    clock_samples,
                    loop_range.start,
                    self.current_speed_multiplier,
                    sample_rate,
                );
                playhead = loop_range.start;
            }
        } else if let Some(stop_at) = self.transport_state.stop_at {
            stop_at_clock_samples = transport.musical_to_samples(
                stop_at,
                self.transport_start_samples,
                self.current_speed_multiplier,
                sample_rate,
            );

            if clock_samples >= stop_at_clock_samples {
                // Stop the transport.
                *self.transport_state.playing = false;
                return ProcTransportInfo {
                    frames,
                    beats_per_minute,
                };
            }
        }

        let mut info = transport.proc_transport_info(
            frames,
            playhead,
            self.current_speed_multiplier,
            sample_rate,
        );

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
                transport: transport.clone(),
                start_clock_samples: self
                    .transport_state
                    .playing
                    .then(|| self.transport_start_samples),
                beats_per_minute: proc_transport_info.beats_per_minute,
                speed_multiplier: self.current_speed_multiplier,
            })
    }

    pub fn shared_clock_info(
        &self,
        clock_samples: InstantSamples,
        sample_rate: NonZeroU32,
        sample_rate_recip: f64,
    ) -> SharedClockInfo {
        self.transport_state
            .transport
            .as_ref()
            .map(|transport| {
                if *self.transport_state.playing {
                    let current_playhead = transport.samples_to_musical(
                        clock_samples,
                        self.transport_start_samples,
                        self.current_speed_multiplier,
                        sample_rate,
                        sample_rate_recip,
                    );

                    SharedClockInfo {
                        current_playhead: Some(current_playhead),
                        transport_is_playing: true,
                        speed_multiplier: self.current_speed_multiplier,
                    }
                } else {
                    SharedClockInfo {
                        current_playhead: Some(self.paused_at_musical_time),
                        transport_is_playing: false,
                        speed_multiplier: self.current_speed_multiplier,
                    }
                }
            })
            .unwrap_or(SharedClockInfo {
                current_playhead: None,
                transport_is_playing: false,
                speed_multiplier: self.current_speed_multiplier,
            })
    }

    pub fn transport_sync_info<'a>(&'a self) -> Option<TransportSyncInfo<'a>> {
        self.transport_state
            .transport
            .as_ref()
            .map(|transport| TransportSyncInfo {
                transport,
                transport_start: self.transport_start_samples,
                speed_multiplier: self.current_speed_multiplier,
            })
    }

    pub fn update_sample_rate(
        &mut self,
        old_sample_rate: NonZeroU32,
        old_sample_rate_recip: f64,
        new_sample_rate: NonZeroU32,
    ) {
        self.transport_start_samples = self
            .transport_start_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
        self.paused_at_clock_samples = self
            .paused_at_clock_samples
            .to_seconds(old_sample_rate, old_sample_rate_recip)
            .to_samples(new_sample_rate);
    }
}

pub(super) struct TransportSyncInfo<'a> {
    pub transport: &'a MusicalTransport,
    pub transport_start: InstantSamples,
    pub speed_multiplier: f64,
}

pub(super) struct SharedClockInfo {
    pub current_playhead: Option<InstantMusical>,
    pub transport_is_playing: bool,
    pub speed_multiplier: f64,
}
