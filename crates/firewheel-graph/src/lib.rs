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
    use crate::backend::BackendProcessInfo;
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

        impl AudioNodeProcessor for DummyClapPluginProcessor {
            fn process(
                &mut self,
                _: &firewheel_core::node::ProcInfo,
                _: Option<firewheel_core::node::ProcBuffers>,
                _: &mut firewheel_core::event::ProcEvents,
                _: &mut firewheel_core::node::ProcExtra,
            ) -> firewheel_core::node::ProcessStatus {
                firewheel_core::node::ProcessStatus::Bypass
            }
        }

        let mut dummy_out_buffer = vec![0.0; 1024];

        let activate_info = ActivateInfo {
            sample_rate: NonZeroU32::new(44100).unwrap(),
            max_block_frames: NonZeroU32::new(1024).unwrap(),
            num_stream_in_channels: 0,
            num_stream_out_channels: 1,
            input_to_output_latency_seconds: 0.0,
        };
        let process_info = BackendProcessInfo {
            num_in_channels: 0,
            num_out_channels: 1,
            frames: 1024,
            process_timestamp: None,
            duration_since_stream_start: Duration::default(),
            input_stream_status: StreamStatus::empty(),
            output_stream_status: StreamStatus::empty(),
            dropped_frames: 0,
            process_to_playback_delay: None,
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

            processor.process_interleaved(&[], &mut dummy_out_buffer, process_info.clone());

            context.remove_node(node_id).unwrap();

            context.update().unwrap();

            processor.process_interleaved(&[], &mut dummy_out_buffer, process_info.clone());

            context.update().unwrap();

            assert!(plugin_main_dropeed.load(Ordering::SeqCst));
        }

        // Test dropping processor before context
        {
            let mut context = FirewheelContext::new(Default::default());
            context.add_node(DummyClapPlugin {}, None).unwrap();

            let mut processor = context.activate(activate_info.clone()).unwrap();

            context.update().unwrap();

            processor.process_interleaved(&[], &mut dummy_out_buffer, process_info.clone());

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

            processor.process_interleaved(&[], &mut dummy_out_buffer, process_info.clone());

            context.update().unwrap();

            context.request_deactivate();

            // The processor must process at least once to deactivate.
            processor.process_interleaved(&[], &mut dummy_out_buffer, process_info.clone());

            let _ = context;
            let _ = processor;
        }
    }
}
