#![cfg_attr(not(feature = "std"), no_std)]

pub mod backend;
mod context;
pub mod error;
pub mod graph;
pub mod processor;

#[cfg(feature = "unsafe_flush_denormals_to_zero")]
mod ftz;

#[cfg(feature = "scheduled_events")]
pub use context::ClearScheduledEventsType;
pub use context::{ActivateInfo, ContextQueue, FirewheelConfig, FirewheelContext, FirewheelFlags};

extern crate alloc;

#[cfg(test)]
mod tests {
    use crate::{backend::BackendProcessInfo, processor::FirewheelProcessor};
    use audioadapter_buffers::direct::InterleavedSlice;
    use bevy_platform::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use core::{num::NonZeroU32, time::Duration};
    use firewheel_core::node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        NodeError, StreamStatus,
    };

    use super::*;

    #[test]
    // Firewheel is designed with
    // [CLAP's threading model](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h)
    // in mind. This allows one to more easily create a custom node that hosts a 3rd party CLAP
    // plugin binary.
    //
    // The purpose of this test is to ensure that the order in which nodes are dropped fit the
    // CLAP threading model.
    fn clap_drop_ordering() {
        struct DummyClapPlugin {}
        struct DummyClapPluginProcessor {
            state: CustomState,
        }
        #[derive(Clone)]
        struct CustomState {
            plugin_main_dropped: Arc<AtomicBool>,
            plugin_processor_dropped: Arc<AtomicBool>,
        }

        impl Drop for DummyClapPluginProcessor {
            fn drop(&mut self) {
                assert!(!self.state.plugin_main_dropped.load(Ordering::SeqCst));
                self.state
                    .plugin_processor_dropped
                    .store(true, Ordering::SeqCst);
            }
        }

        impl Drop for CustomState {
            fn drop(&mut self) {
                assert!(self.plugin_processor_dropped.load(Ordering::SeqCst));
                self.plugin_main_dropped.store(true, Ordering::SeqCst);
            }
        }

        impl AudioNode for DummyClapPlugin {
            type Configuration = EmptyConfig;

            fn info(&self, _: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
                Ok(AudioNodeInfo::new().custom_state(CustomState {
                    plugin_main_dropped: Arc::new(AtomicBool::new(false)),
                    plugin_processor_dropped: Arc::new(AtomicBool::new(false)),
                }))
            }

            fn construct_processor(
                &self,
                _: &Self::Configuration,
                cx: ConstructProcessorContext,
            ) -> Result<impl AudioNodeProcessor, NodeError> {
                let state = cx.custom_state::<CustomState>().unwrap().clone();

                Ok(DummyClapPluginProcessor { state })
            }
        }

        impl AudioNodeProcessor for DummyClapPluginProcessor {}

        const DUMMY_OUT_LEN: usize = 1024;
        let mut dummy_out_buffer = vec![0.0; DUMMY_OUT_LEN];

        let activate_info = ActivateInfo {
            sample_rate: NonZeroU32::new(44100).unwrap(),
            max_block_frames: NonZeroU32::new(DUMMY_OUT_LEN as u32).unwrap(),
            num_stream_in_channels: 0,
            num_stream_out_channels: 1,
            input_to_output_latency_seconds: 0.0,
        };
        let process_info = BackendProcessInfo {
            frames: DUMMY_OUT_LEN,
            process_timestamp: None,
            duration_since_stream_start: Duration::default(),
            input_stream_status: StreamStatus::empty(),
            output_stream_status: StreamStatus::empty(),
            dropped_frames: 0,
            process_to_playback_delay: None,
        };

        let mut process = |processor: &mut FirewheelProcessor| {
            processor.process(
                &InterleavedSlice::new(&[], 0, 0).unwrap(),
                &mut InterleavedSlice::new_mut(&mut dummy_out_buffer, 1, DUMMY_OUT_LEN).unwrap(),
                process_info.clone(),
            );
        };

        // Test dropping by removing node manually
        {
            let mut context = FirewheelContext::new(Default::default());
            let node_id = context.add_node(DummyClapPlugin {}, None).unwrap();

            let plugin_main_dropeed = context
                .node_state::<CustomState>(node_id)
                .unwrap()
                .plugin_main_dropped
                .clone();

            let mut processor = context.activate(activate_info.clone()).unwrap();

            context.update().unwrap();

            process(&mut processor);

            context.remove_node(node_id).unwrap();

            context.update().unwrap();

            process(&mut processor);

            context.update().unwrap();

            assert!(plugin_main_dropeed.load(Ordering::SeqCst));
        }

        // Test dropping processor before context
        {
            let mut context = FirewheelContext::new(Default::default());
            context.add_node(DummyClapPlugin {}, None).unwrap();

            let mut processor = context.activate(activate_info.clone()).unwrap();

            context.update().unwrap();

            process(&mut processor);

            context.update().unwrap();

            let _ = processor;
            let _ = context;
        }

        // Test dropping processor after context
        {
            let mut context = FirewheelContext::new(Default::default());
            context.add_node(DummyClapPlugin {}, None).unwrap();

            let mut processor = context.activate(activate_info.clone()).unwrap();

            context.update().unwrap();

            process(&mut processor);

            context.update().unwrap();

            context.request_deactivate();

            // The processor must process at least once to deactivate.
            process(&mut processor);

            let _ = context;
            let _ = processor;
        }
    }
}
