use firewheel_core::{
    clock::TransportState,
    dsp::{buffer::ChannelBuffer, declick::DeclickValues},
    StreamInfo,
};
use ringbuf::traits::{Consumer, Producer};

use crate::{
    backend::AudioBackend,
    graph::{NodeHeapData, ScheduleHeapData},
    processor::{
        ContextToProcessorMsg, FirewheelProcessorInner, NodeEntry, NodeEventSchedulerData,
        ProcessorToContextMsg,
    },
};

impl<B: AudioBackend> FirewheelProcessorInner<B> {
    pub fn poll_messages(&mut self) {
        while let Some(msg) = self.from_graph_rx.try_pop() {
            match msg {
                ContextToProcessorMsg::EventGroup(mut event_group) => {
                    self.event_scheduler.push_event_group(
                        &mut event_group,
                        &mut self.nodes,
                        self.sample_rate,
                        &self.proc_transport_state,
                    );

                    let _ = self
                        .to_graph_tx
                        .try_push(ProcessorToContextMsg::ReturnEventGroup(event_group));
                }
                ContextToProcessorMsg::NewSchedule(new_schedule_data) => {
                    self.new_schedule(new_schedule_data);
                }
                ContextToProcessorMsg::HardClipOutputs(hard_clip_outputs) => {
                    self.hard_clip_outputs = hard_clip_outputs;
                }
                ContextToProcessorMsg::SetTransportState(new_transport_state) => {
                    self.set_transport_state(new_transport_state);
                }
                ContextToProcessorMsg::ClearScheduledEvents(msgs) => {
                    self.event_scheduler
                        .handle_clear_scheduled_events_event(&msgs, &mut self.nodes);

                    let _ = self
                        .to_graph_tx
                        .try_push(ProcessorToContextMsg::ReturnClearScheduledEvents(msgs));
                }
            }
        }
    }

    fn new_schedule(&mut self, mut new_schedule_data: Box<ScheduleHeapData>) {
        assert_eq!(
            new_schedule_data.schedule.max_block_frames(),
            self.max_block_frames
        );

        if let Some(new_arena) = &mut new_schedule_data.new_node_arena {
            // A new arena with a larger allocated capacity was sent.

            for (index, node_entry) in self.nodes.drain() {
                let _ = new_arena.insert_at(index, node_entry);
            }

            core::mem::swap(&mut self.nodes, new_arena);
        }

        let mut remove_old_scheduled_events = false;
        if let Some(mut old_schedule_data) = self.schedule_data.take() {
            core::mem::swap(
                &mut old_schedule_data.removed_nodes,
                &mut new_schedule_data.removed_nodes,
            );

            for node_id in new_schedule_data.nodes_to_remove.iter() {
                if let Some(node_entry) = self.nodes.remove(node_id.0) {
                    if self.event_scheduler.node_has_scheduled_events(&node_entry) {
                        remove_old_scheduled_events = true;
                    }

                    old_schedule_data.removed_nodes.push(NodeHeapData {
                        id: *node_id,
                        processor: node_entry.processor,
                    });
                }
            }

            let _ = self
                .to_graph_tx
                .try_push(ProcessorToContextMsg::ReturnSchedule(old_schedule_data));
        }

        for n in new_schedule_data.new_node_processors.drain(..) {
            assert!((n.id.0.slot() as usize) < self.nodes.capacity());

            assert!(self
                .nodes
                .insert_at(
                    n.id.0,
                    NodeEntry {
                        processor: n.processor,
                        event_data: NodeEventSchedulerData::default(),
                    }
                )
                .is_none());
        }

        if remove_old_scheduled_events {
            self.event_scheduler
                .remove_events_from_removed_nodes(&self.nodes);
        }

        self.schedule_data = Some(new_schedule_data);
    }

    fn set_transport_state(&mut self, new_transport_state: Box<TransportState>) {
        let old_transport_state = self.proc_transport_state.set_transport_state(
            new_transport_state,
            self.clock_samples,
            self.sample_rate,
            self.sample_rate_recip,
        );

        self.event_scheduler.sync_scheduled_events(
            self.proc_transport_state
                .transport_and_start_clock_samples(),
            self.sample_rate,
        );

        let _ = self
            .to_graph_tx
            .try_push(ProcessorToContextMsg::ReturnTransportState(
                old_transport_state,
            ));
    }

    pub fn stream_stopped(&mut self) {
        self.sync_shared_clock(None);

        for (_, node) in self.nodes.iter_mut() {
            node.processor.stream_stopped();
        }
    }

    /// Called when a new audio stream has been started to replace the old one.
    ///
    /// Note, this method gets called on the main thread, not the audio thread.
    pub fn new_stream(&mut self, stream_info: &StreamInfo) {
        for (_, node) in self.nodes.iter_mut() {
            node.processor.new_stream(stream_info);
        }

        if self.sample_rate != stream_info.sample_rate {
            self.clock_samples = self
                .clock_samples
                .to_seconds(self.sample_rate, self.sample_rate_recip)
                .to_samples(stream_info.sample_rate);

            self.proc_transport_state.update_sample_rate(
                self.sample_rate,
                self.sample_rate_recip,
                stream_info.sample_rate,
            );

            self.event_scheduler.sample_rate_changed(
                self.sample_rate,
                self.sample_rate_recip,
                stream_info.sample_rate,
            );

            self.sample_rate = stream_info.sample_rate;
            self.sample_rate_recip = stream_info.sample_rate_recip;

            self.declick_values = DeclickValues::new(stream_info.declick_frames);
        }

        if self.max_block_frames != stream_info.max_block_frames.get() as usize {
            self.max_block_frames = stream_info.max_block_frames.get() as usize;

            self.scratch_buffers = ChannelBuffer::new(stream_info.max_block_frames.get() as usize);
        }
    }
}
