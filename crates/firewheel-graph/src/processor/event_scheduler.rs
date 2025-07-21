use std::{num::NonZeroU32, ops::Range};

use arrayvec::ArrayVec;
use firewheel_core::{
    clock::{DurationSamples, EventInstant, InstantSamples, MusicalTransport},
    event::{NodeEvent, NodeEventList, NodeEventListIndex},
    node::{NodeID, ProcBuffers, ProcInfo},
};
use thunderdome::Arena;

use crate::{
    context::ClearScheduledEventsType,
    processor::{BufferOutOfSpaceMode, ClearScheduledEventsEvent, NodeEntry, ProcTransportState},
};

const MAX_CLUMP_INDICES: usize = 8;

pub(super) struct EventScheduler {
    immediate_event_buffer: Vec<Option<NodeEvent>>,
    immediate_event_buffer_capacity: usize,

    // A slab allocator arena for scheduled node events.
    scheduled_event_arena: Vec<Option<NodeEvent>>,
    scheduled_event_arena_free_slots: Vec<u32>,

    // Sorting this Vec is much faster than sorting `scheduled_event_arena`
    // directly since its data type is smaller and it implements `Copy`.
    sorted_event_buffer_indices: Vec<(u32, InstantSamples)>,
    scheduled_events_need_sorting: bool,
    num_elapsed_sorted_events: usize,

    num_scheduled_musical_events: usize,
    num_scheduled_non_musical_events: usize,

    buffer_out_of_space_mode: BufferOutOfSpaceMode,
}

impl EventScheduler {
    pub fn new(
        immediate_event_buffer_capacity: usize,
        scheduled_event_buffer_capacity: usize,
        buffer_out_of_space_mode: BufferOutOfSpaceMode,
    ) -> Self {
        let mut scheduled_event_arena = Vec::new();
        scheduled_event_arena.resize_with(scheduled_event_buffer_capacity, || None);

        Self {
            immediate_event_buffer: Vec::with_capacity(immediate_event_buffer_capacity),
            immediate_event_buffer_capacity,

            scheduled_event_arena,
            scheduled_event_arena_free_slots: (0..scheduled_event_buffer_capacity as u32)
                .rev()
                .collect(),

            sorted_event_buffer_indices: Vec::with_capacity(scheduled_event_buffer_capacity),
            scheduled_events_need_sorting: false,
            num_scheduled_non_musical_events: 0,

            num_elapsed_sorted_events: 0,
            num_scheduled_musical_events: 0,

            buffer_out_of_space_mode,
        }
    }

    pub fn push_event_group(
        &mut self,
        event_group: &mut Vec<NodeEvent>,
        nodes: &mut Arena<NodeEntry>,
        sample_rate: NonZeroU32,
        proc_transport_state: &ProcTransportState,
    ) {
        self.truncate_elapsed_events();

        for event in event_group.drain(..) {
            if let Some(node_entry) = nodes.get_mut(event.node_id.0) {
                self.push_event(
                    event,
                    &mut node_entry.event_data,
                    sample_rate,
                    proc_transport_state,
                );
            }
        }
    }

    fn push_event(
        &mut self,
        event: NodeEvent,
        node_data: &mut NodeEventSchedulerData,
        sample_rate: NonZeroU32,
        proc_transport_state: &ProcTransportState,
    ) {
        if let Some(event_instant) = event.time {
            let slot = if let Some(slot) = self.scheduled_event_arena_free_slots.pop() {
                slot
            } else {
                let drop_event = self.extend_scheduled_event_buffer();
                if drop_event {
                    return;
                }

                self.scheduled_event_arena_free_slots.pop().unwrap()
            };

            let time_samples = match event_instant {
                EventInstant::Samples(samples) => {
                    self.num_scheduled_non_musical_events += 1;
                    node_data.num_scheduled_non_musical_events += 1;

                    samples
                }
                EventInstant::Seconds(seconds) => {
                    self.num_scheduled_non_musical_events += 1;
                    node_data.num_scheduled_non_musical_events += 1;

                    seconds.to_samples(sample_rate)
                }
                EventInstant::Musical(musical) => {
                    self.num_scheduled_musical_events += 1;
                    node_data.num_scheduled_musical_events += 1;

                    // Set to `InstantSamples::MAX` to "unschedule" the event.
                    proc_transport_state
                        .musical_to_samples(musical, sample_rate)
                        .unwrap_or(InstantSamples::MAX)
                }
            };

            if !self.scheduled_events_need_sorting {
                if let Some((_, last_instant)) = self.sorted_event_buffer_indices.last() {
                    if time_samples > *last_instant {
                        self.scheduled_events_need_sorting = true;
                    }
                }
            }

            self.scheduled_event_arena[slot as usize] = Some(event);

            self.sorted_event_buffer_indices.push((slot, time_samples));
        } else {
            if self.immediate_event_buffer.len() == self.immediate_event_buffer_capacity {
                match self.buffer_out_of_space_mode {
                    BufferOutOfSpaceMode::AllocateOnAudioThread => {
                        // TODO: Realtime-safe logging
                        log::warn!("Firewheel immediate event buffer is full! Please increase FirewheelConfig::immediate_event_capacity to avoid allocations on the audio thread.");
                    }
                    BufferOutOfSpaceMode::Panic => {
                        panic!("Firewheel immediate event buffer is full! Please increase FirewheelConfig::immediate_event_capacity.");
                    }
                    BufferOutOfSpaceMode::DropEvents => {
                        // TODO: Realtime-safe logging
                        log::warn!(
                            "Firewheel immediate event buffer is full and event was dropped! Please increase FirewheelConfig::immediate_event_capacity."
                        );
                        return;
                    }
                }
            }

            // Because immediate events for a node are likely to be clumped together,
            // the linear search is optimized by storing the starting index of each
            // new clump.
            let is_new_clump = self
                .immediate_event_buffer
                .last()
                .map(|prev_event| prev_event.as_ref().unwrap().node_id != event.node_id)
                .unwrap_or(true);
            if is_new_clump {
                let _ = node_data
                    .immediate_event_clump_indices
                    .try_push(self.immediate_event_buffer.len() as u32);
            }

            node_data.num_immediate_events += 1;

            self.immediate_event_buffer.push(Some(event));
        }
    }

    pub fn node_has_scheduled_events(&self, node_entry: &NodeEntry) -> bool {
        node_entry.event_data.num_scheduled_musical_events > 0
            || node_entry.event_data.num_scheduled_non_musical_events > 0
    }

    pub fn remove_events_from_removed_nodes(&mut self, nodes: &Arena<NodeEntry>) {
        self.truncate_elapsed_events();

        self.sorted_event_buffer_indices.retain(|(event_i, _)| {
            let event = self.scheduled_event_arena[*event_i as usize]
                .as_ref()
                .unwrap();

            if nodes.contains(event.node_id.0) {
                true
            } else {
                if event.time.unwrap().is_musical() {
                    self.num_scheduled_musical_events -= 1;
                } else {
                    self.num_scheduled_non_musical_events -= 1;
                }

                // Clear any `ArcGc`s this event may have had.
                self.scheduled_event_arena[*event_i as usize] = None;

                self.scheduled_event_arena_free_slots.push(*event_i);

                false
            }
        });
    }

    pub fn sync_scheduled_events(
        &mut self,
        transport_and_start_clock_samples: Option<(&MusicalTransport, InstantSamples)>,
        sample_rate: NonZeroU32,
    ) {
        if self.num_scheduled_musical_events == 0 {
            return;
        }

        self.truncate_elapsed_events();

        if let Some((transport, start_clock_samples)) = transport_and_start_clock_samples {
            for (event_i, time_samples) in self.sorted_event_buffer_indices.iter_mut() {
                let event = self.scheduled_event_arena[*event_i as usize]
                    .as_ref()
                    .unwrap();

                if let Some(EventInstant::Musical(musical)) = event.time {
                    *time_samples =
                        transport.musical_to_samples(musical, start_clock_samples, sample_rate);
                }
            }
        } else {
            for (event_i, time_samples) in self.sorted_event_buffer_indices.iter_mut() {
                let event = self.scheduled_event_arena[*event_i as usize]
                    .as_ref()
                    .unwrap();

                if let Some(EventInstant::Musical(_)) = event.time {
                    // Set to `MAX` to effectively de-schedule the event.
                    *time_samples = InstantSamples::MAX;
                }
            }
        }

        self.scheduled_events_need_sorting = true;
    }

    pub fn handle_clear_scheduled_events_event(
        &mut self,
        msgs: &[ClearScheduledEventsEvent],
        nodes: &mut Arena<NodeEntry>,
    ) {
        self.truncate_elapsed_events();

        // TODO: This could be optimized by doing a single linear search and
        // a hash set.
        for msg in msgs.iter() {
            if let Some(node_id) = msg.node_id {
                let Some(node_entry) = nodes.get(node_id.0) else {
                    continue;
                };

                match msg.event_type {
                    ClearScheduledEventsType::All => {
                        if node_entry.event_data.num_scheduled_musical_events == 0
                            && node_entry.event_data.num_scheduled_non_musical_events == 0
                        {
                            continue;
                        }
                    }
                    ClearScheduledEventsType::MusicalOnly => {
                        if node_entry.event_data.num_scheduled_musical_events == 0 {
                            continue;
                        }
                    }
                    ClearScheduledEventsType::NonMusicalOnly => {
                        if node_entry.event_data.num_scheduled_non_musical_events == 0 {
                            continue;
                        }
                    }
                }
            } else {
                // Else `None` means to clear scheduled events for all nodes.
                match msg.event_type {
                    ClearScheduledEventsType::All => {
                        if self.num_scheduled_musical_events == 0
                            && self.num_scheduled_non_musical_events == 0
                        {
                            continue;
                        }
                    }
                    ClearScheduledEventsType::MusicalOnly => {
                        if self.num_scheduled_musical_events == 0 {
                            continue;
                        }
                    }
                    ClearScheduledEventsType::NonMusicalOnly => {
                        if self.num_scheduled_non_musical_events == 0 {
                            continue;
                        }
                    }
                }
            }

            self.sorted_event_buffer_indices.retain(|(event_i, _)| {
                let event = self.scheduled_event_arena[*event_i as usize]
                    .as_ref()
                    .unwrap();

                if let Some(node_id) = msg.node_id {
                    if event.node_id != node_id {
                        return true;
                    }
                }
                // Else `None` means to remove scheduled events for all nodes.

                if event.time.unwrap().is_musical() {
                    if let ClearScheduledEventsType::NonMusicalOnly = msg.event_type {
                        return true;
                    }

                    self.num_scheduled_musical_events -= 1;
                    nodes[event.node_id.0]
                        .event_data
                        .num_scheduled_musical_events -= 1;
                } else {
                    if let ClearScheduledEventsType::MusicalOnly = msg.event_type {
                        return true;
                    }

                    self.num_scheduled_non_musical_events -= 1;
                    nodes[event.node_id.0]
                        .event_data
                        .num_scheduled_non_musical_events -= 1;
                }

                // Clear any `ArcGc`s this event may have had.
                self.scheduled_event_arena[*event_i as usize] = None;

                self.scheduled_event_arena_free_slots.push(*event_i);

                false
            });
        }
    }

    pub fn sample_rate_changed(
        &mut self,
        old_sample_rate: NonZeroU32,
        old_sample_rate_recip: f64,
        new_sample_rate: NonZeroU32,
    ) {
        for (_, time_samples) in self.sorted_event_buffer_indices.iter_mut() {
            if *time_samples != InstantSamples::MAX {
                *time_samples = time_samples
                    .to_seconds(old_sample_rate, old_sample_rate_recip)
                    .to_samples(new_sample_rate);
            }
        }
    }

    /// Find scheduled events that have elapsed this processing block
    pub fn prepare_process_block(&mut self, proc_info: &ProcInfo, nodes: &mut Arena<NodeEntry>) {
        self.sort_events();

        let end_samples = proc_info.clock_samples_range().end;

        for (sorted_i, (event_i, time_samples)) in self
            .sorted_event_buffer_indices
            .iter()
            .enumerate()
            .skip(self.num_elapsed_sorted_events)
        {
            if *time_samples < end_samples {
                let event = self.scheduled_event_arena[*event_i as usize]
                    .as_ref()
                    .unwrap();

                if event.time.unwrap().is_musical() {
                    self.num_scheduled_musical_events -= 1;
                } else {
                    self.num_scheduled_non_musical_events -= 1;
                }

                self.scheduled_event_arena_free_slots.push(*event_i);

                if let Some(node_entry) = nodes.get_mut(event.node_id.0) {
                    if node_entry.event_data.num_scheduled_events_this_block == 0 {
                        // Optimize the linear search a bit by starting at the index
                        // of the first known scheduled event for this node.
                        node_entry.event_data.first_sorted_event_index = sorted_i;
                    }

                    // Keep track of the number of elapsed schedueld events this
                    // block to further optimize the linear search.
                    node_entry.event_data.num_scheduled_events_this_block += 1;
                } else {
                    self.scheduled_event_arena[*event_i as usize] = None;
                }

                self.num_elapsed_sorted_events += 1;
            } else {
                // The event happens after this processing block, so we are done
                // searching.
                break;
            }
        }
    }

    /// Process in sub-chunks for each new scheduled event (or process a single
    /// chunk if there are no scheduled events).
    pub fn process_node(
        &mut self,
        node_id: NodeID,
        node_entry: &mut NodeEntry,
        block_frames: usize,
        clock_samples: InstantSamples,
        proc_info: &mut ProcInfo,
        node_event_queue: &mut Vec<NodeEventListIndex>,
        mut proc_buffers: ProcBuffers,
        mut on_sub_chunk: impl FnMut(
            SubChunkInfo,
            &mut NodeEntry,
            &mut ProcInfo,
            &mut NodeEventList,
            &mut ProcBuffers,
        ),
    ) {
        let push_event = |node_event_queue: &mut Vec<NodeEventListIndex>,
                          event: NodeEventListIndex| {
            if node_event_queue.len() == node_event_queue.capacity() {
                match self.buffer_out_of_space_mode {
                    BufferOutOfSpaceMode::AllocateOnAudioThread => {
                        // TODO: realtime safe logging
                        log::warn!("Firewheel event queue is full! Please increase FirewheelConfig::event_queue_capacity to avoid allocations on the audio thread.");
                    }
                    BufferOutOfSpaceMode::Panic => {
                        panic!("Firewheel event queue is full! Please increase FirewheelConfig::event_queue_capacity.");
                    }
                    BufferOutOfSpaceMode::DropEvents => {
                        // TODO: realtime safe logging
                        log::warn!("Firewheel event queue is full and event was dropped! Please increase FirewheelConfig::event_queue_capacity.");
                    }
                }
            }

            node_event_queue.push(event);
        };

        // Optimize the linear search a bit by starting at the index of the
        // first known scheduled event for this node.
        let mut sorted_event_i = node_entry.event_data.first_sorted_event_index;

        let mut sub_clock_samples = clock_samples;
        let mut frames_processed = 0;
        while frames_processed < block_frames {
            let mut sub_chunk_frames = block_frames - frames_processed;

            // Add scheduled events to the processing queue.
            let mut upcoming_event_i = None;
            while node_entry.event_data.num_scheduled_events_this_block > 0 {
                let (event_i, time_samples) = self.sorted_event_buffer_indices[sorted_event_i];
                let event = self.scheduled_event_arena[event_i as usize]
                    .as_ref()
                    .unwrap();

                sorted_event_i += 1;

                if event.node_id != node_id {
                    continue;
                }

                node_entry.event_data.num_scheduled_events_this_block -= 1;
                if event.time.unwrap().is_musical() {
                    node_entry.event_data.num_scheduled_musical_events -= 1;
                } else {
                    node_entry.event_data.num_scheduled_non_musical_events -= 1;
                }

                if time_samples <= sub_clock_samples {
                    // If the scheduled event elapses on or before the start of this
                    // sub-chunk, add it to the processing queue.
                    push_event(node_event_queue, NodeEventListIndex::Scheduled(event_i));
                } else {
                    // Else set the length of this sub-chunk to process up to this event.
                    // Once this sub-chunk has been processed, add it to the processing
                    // queue for the next sub-chunk.
                    sub_chunk_frames =
                        ((time_samples - sub_clock_samples).0 as usize).min(sub_chunk_frames);
                    upcoming_event_i = Some(event_i);

                    break;
                }
            }

            // If this is the first (or only) sub-chunk, add all of the immediate events
            // to the processing queue.
            //
            // Because immediate events for a node are likely to be clumped together,
            // the linear search is optimized by storing the starting index of each new
            // clump.
            //
            // Note, this is done after the scheduled events because immediate events
            // take priority in determining the final state of a node's parameters.
            for (clump_i, clump_event_start_i) in node_entry
                .event_data
                .immediate_event_clump_indices
                .iter()
                .enumerate()
            {
                push_event(
                    node_event_queue,
                    NodeEventListIndex::Immediate(*clump_event_start_i),
                );

                node_entry.event_data.num_immediate_events -= 1;
                if node_entry.event_data.num_immediate_events == 0 {
                    break;
                }

                for (event_i, event) in self
                    .immediate_event_buffer
                    .iter()
                    .enumerate()
                    .skip(*clump_event_start_i as usize + 1)
                    .filter_map(|(event_i, event)| event.as_ref().map(|event| (event_i, event)))
                {
                    if event.node_id == node_id {
                        push_event(
                            node_event_queue,
                            NodeEventListIndex::Immediate(event_i as u32),
                        );

                        node_entry.event_data.num_immediate_events -= 1;
                        if node_entry.event_data.num_immediate_events == 0 {
                            break;
                        }
                    } else if clump_i
                        != node_entry.event_data.immediate_event_clump_indices.len() - 1
                    {
                        break;
                    }
                }
            }
            node_entry.event_data.immediate_event_clump_indices.clear();

            let mut node_event_list = NodeEventList::new(
                &mut self.immediate_event_buffer,
                &mut self.scheduled_event_arena,
                node_event_queue,
            );

            (on_sub_chunk)(
                SubChunkInfo {
                    sub_chunk_range: frames_processed..frames_processed + sub_chunk_frames,
                    sub_clock_samples,
                },
                node_entry,
                proc_info,
                &mut node_event_list,
                &mut proc_buffers,
            );

            // Ensure that all `ArcGc`s have been cleaned up.
            for event in node_event_list.drain() {
                let _ = event;
            }

            node_event_queue.clear();

            // If there was an upcoming scheduled event, add it to the processing queue
            // for the next sub-chunk.
            if let Some(event_i) = upcoming_event_i {
                // Sanity check. There should be no upcoming event if this is the last
                // sub-chunk.
                assert_ne!(frames_processed + sub_chunk_frames, block_frames);

                push_event(node_event_queue, NodeEventListIndex::Scheduled(event_i));
            }

            // Advance to the next sub-chunk.
            frames_processed += sub_chunk_frames;
            sub_clock_samples += DurationSamples(sub_chunk_frames as i64);
        }

        // Sanity check. There should be no scheduled events left.
        assert_eq!(node_entry.event_data.num_scheduled_events_this_block, 0);
    }

    /// Clean up event buffers
    pub fn cleanup_process_block(&mut self) {
        self.immediate_event_buffer.clear();
    }

    fn sort_events(&mut self) {
        if !self.scheduled_events_need_sorting {
            return;
        }
        self.scheduled_events_need_sorting = false;

        self.truncate_elapsed_events();

        // TODO: While sorting here on the audio thread is fine for the general use
        // case of having only a handful of scheduled events, if the user has
        // scheduled hundreds or even thousands of events (i.e. they have scheduled
        // a full music sequence), this may not be the best choice.
        self.sorted_event_buffer_indices
            .sort_unstable_by_key(|(_, time_samples)| *time_samples);
    }

    /// Truncate elapsed event slots from the sorted event buffer.
    fn truncate_elapsed_events(&mut self) {
        if self.num_elapsed_sorted_events == 0 {
            return;
        }

        self.sorted_event_buffer_indices
            .copy_within(self.num_elapsed_sorted_events.., 0);
        self.sorted_event_buffer_indices.resize(
            self.sorted_event_buffer_indices.len() - self.num_elapsed_sorted_events,
            Default::default(),
        );

        self.num_elapsed_sorted_events = 0;
    }

    /// Returns `true` if the event should be dropped.
    fn extend_scheduled_event_buffer(&mut self) -> bool {
        match self.buffer_out_of_space_mode {
            BufferOutOfSpaceMode::AllocateOnAudioThread => {
                // TODO: Realtime-safe logging
                log::warn!("Firewheel scheduled event buffer is full! Please increase FirewheelConfig::scheduled_event_capacity to avoid allocations on the audio thread.");

                let old_len = self.scheduled_event_arena.len();

                self.scheduled_event_arena.resize_with(old_len * 2, || None);

                for i in (old_len as u32..(old_len * 2) as u32).rev() {
                    self.scheduled_event_arena_free_slots.push(i);
                }

                self.sorted_event_buffer_indices.reserve(old_len);

                false
            }
            BufferOutOfSpaceMode::Panic => {
                panic!("Firewheel scheduled event buffer is full! Please increase FirewheelConfig::scheduled_event_capacity.");
            }
            BufferOutOfSpaceMode::DropEvents => {
                // TODO: Realtime-safe logging
                log::warn!("Firewheel scheduled event buffer is full and event was dropped! Please increase FirewheelConfig::scheduled_event_capacity.");
                true
            }
        }
    }
}

#[derive(Default)]
pub(super) struct NodeEventSchedulerData {
    num_immediate_events: usize,
    /// The index of the first event in a clump of events for this node.
    /// Events for a single node are likely to be clumped together.
    immediate_event_clump_indices: ArrayVec<u32, MAX_CLUMP_INDICES>,

    num_scheduled_musical_events: usize,
    num_scheduled_non_musical_events: usize,

    num_scheduled_events_this_block: usize,
    first_sorted_event_index: usize,
}

pub(super) struct SubChunkInfo {
    pub sub_chunk_range: Range<usize>,
    pub sub_clock_samples: InstantSamples,
}
