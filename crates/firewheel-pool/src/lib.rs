#![cfg_attr(not(feature = "std"), no_std)]

use core::num::NonZeroUsize;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use firewheel_core::{
    channel_config::NonZeroChannelCount,
    node::{AudioNode, NodeID},
};
use firewheel_graph::{
    ContextQueue, FirewheelContext,
    error::{AddEdgeError, CompileGraphError, RemoveNodeError, UpdateError},
    graph::Edge,
};
use smallvec::SmallVec;
use thunderdome::Arena;

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;
use firewheel_core::node::NodeError;

#[cfg(feature = "sampler")]
mod sampler;
#[cfg(feature = "sampler")]
pub use sampler::SamplerPool;

mod volume;
mod volume_pan;
pub use volume::VolumeChain;
pub use volume_pan::VolumePanChain;

#[cfg(feature = "spatial_basic")]
mod spatial_basic;
#[cfg(feature = "spatial_basic")]
pub use spatial_basic::SpatialBasicChain;

#[cfg(feature = "sampler")]
pub type SamplerPoolVolumePan = AudioNodePool<SamplerPool, VolumePanChain>;
#[cfg(all(feature = "sampler", feature = "spatial_basic"))]
pub type SamplerPoolSpatialBasic = AudioNodePool<SamplerPool, SpatialBasicChain>;

/// Information about the input/output nodes for an [`FxChain`] instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxChainIo {
    /// The ID of the first node in this worker instance. (i.e. the sampler node)
    pub first_node_id: NodeID,
    /// The number of output channels in the first node.
    pub first_node_out_channels: NonZeroChannelCount,
    /// The ID of the node that the last node in this FX chain instance should connect
    /// to.
    pub dst_node_id: NodeID,
    /// The number of input channels in `dst_node_id`.
    pub dst_node_in_channels: NonZeroChannelCount,
}

/// A trait describing an "FX chain" for use in an [`AudioNodePool`].
pub trait FxChain: Default {
    /// The one-time configuration for constructing a new instance of this fx chain.
    ///
    /// When no configuration is required, [`EmptyConfig`](firewheel_core::node::EmptyConfig)
    /// should be used.
    type Configuration: Default;

    /// Construct the nodes in the FX chain and connect them, returning a list of the
    /// new node ids.
    ///
    /// * `config` - The configuration of this fx chain instance.
    /// * `io` Information about the input/output nodes for this fx chain instance.
    /// * `cx` - The firewheel context.
    fn construct_and_connect(
        &mut self,
        configuration: &Self::Configuration,
        io: &FxChainIo,
        cx: &mut FirewheelContext,
    ) -> Result<Vec<NodeID>, ModifyNodePoolError>;
}

struct Worker<N: PoolableNode, FX: FxChain> {
    first_node_params: N::AudioNode,
    first_node_id: NodeID,
    additional_state: N::AdditionalNodeState,

    fx_state: FxChainState<FX>,

    assigned_worker_id: Option<WorkerID>,
}

impl<N: PoolableNode, FX: FxChain> Worker<N, FX> {
    fn remove_nodes(
        self,
        cx: &mut FirewheelContext,
        removed_nodes: &mut Vec<NodeID>,
        removed_edges: &mut Vec<Edge>,
    ) {
        if let Ok(edges) = cx.remove_node(self.first_node_id) {
            removed_nodes.push(self.first_node_id);
            removed_edges.extend_from_slice(&edges);
        }

        for node_id in self.fx_state.node_ids {
            if let Ok(edges) = cx.remove_node(node_id) {
                removed_nodes.push(node_id);
                removed_edges.extend_from_slice(&edges);
            }
        }
    }
}

#[derive(Debug)]
pub struct FxChainState<FX: FxChain> {
    pub fx_chain: FX,
    pub node_ids: Vec<NodeID>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerID(thunderdome::Index);

impl WorkerID {
    pub const DANGLING: Self = Self(thunderdome::Index::DANGLING);
}

impl Default for WorkerID {
    fn default() -> Self {
        Self::DANGLING
    }
}

/// A trait describing the first node in an [`AudioNodePool`].
pub trait PoolableNode {
    /// The node parameters
    type AudioNode: AudioNode + Clone + 'static;
    /// Additional state to store for each node
    type AdditionalNodeState: Default + 'static;

    /// Return `true` if the given parameters signify that the sequence is stopped,
    /// `false` otherwise.
    fn params_stopped(params: &Self::AudioNode) -> bool;
    /// Return `true` if the node state of the given node is stopped.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn node_is_stopped(
        node_id: NodeID,
        params: &Self::AudioNode,
        additional_state: &mut Self::AdditionalNodeState,
        cx: &mut FirewheelContext,
    ) -> Result<bool, PoolError>;

    /// Return a score of how ready this node is to accept new work.
    ///
    /// The worker with the highest worker score will be chosen for the new work.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn worker_score(
        params: &Self::AudioNode,
        node_id: NodeID,
        additional_state: &mut Self::AdditionalNodeState,
        cx: &mut FirewheelContext,
    ) -> Result<u64, PoolError>;

    /// Diff the new parameters and push the changes into the event queue.
    fn diff(
        baseline: &Self::AudioNode,
        new: &Self::AudioNode,
        additional_state: &mut Self::AdditionalNodeState,
        event_queue: &mut ContextQueue,
    );

    /// Notify the node state that a sequence is playing.
    ///
    /// This is used to account for the delay between sending an event to the node
    /// and the node receiving the event.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn mark_playing(
        node_id: NodeID,
        params: &Self::AudioNode,
        additional_state: &mut Self::AdditionalNodeState,
        cx: &mut FirewheelContext,
    ) -> Result<(), PoolError>;

    /// Pause the sequence in the node parameters
    fn pause(params: &mut Self::AudioNode, additional_state: &mut Self::AdditionalNodeState);
    /// Resume the sequence in the node parameters
    fn resume(params: &mut Self::AudioNode, additional_state: &mut Self::AdditionalNodeState);
    /// Stop the sequence in the node parameters
    fn stop(params: &mut Self::AudioNode, additional_state: &mut Self::AdditionalNodeState);
}

/// A pool of audio node chains that can dynamically be assigned work.
pub struct AudioNodePool<N: PoolableNode, FX: FxChain> {
    workers: Vec<Worker<N, FX>>,
    worker_ids: Arena<usize>,
    num_active_workers: usize,
    dst_node_id: NodeID,
}

impl<N: PoolableNode, FX: FxChain> AudioNodePool<N, FX>
where
    <N::AudioNode as AudioNode>::Configuration: Clone,
{
    /// Construct a new sampler pool.
    ///
    /// * `num_workers` - The total number of workers that can work in parallel. More workers
    ///   will allow more samples to be played concurrently, but will also increase processing
    ///   overhead. A value of `16` is a good place to start.
    /// * `first_node` - The state of the first node in each FX chain instance.
    /// * `first_node_config` - The configuration of the first node in each FX chain instance.
    ///   Set to `None` to use the default configuration.
    /// * `fx_chain_config` - The configuration of each fx chain instance. Set to `None` to
    ///   use the default configuration.
    /// * `dst_node_id` - The ID of the node that the last effect in each fx chain instance
    ///   will connect to.
    /// * `call_update_when_done` - If `true`, then the [`FirewheelContext::update()`] will
    ///   be called after all new nodes have been added. This can be used to ensure that all
    ///   nodes activate without errors before committing the changes to the audio graph.
    /// * `cx` - The firewheel context.
    ///
    /// If an error is returned, then the audio graph has not been modified.
    pub fn new(
        num_workers: NonZeroUsize,
        first_node: N::AudioNode,
        first_node_config: Option<<N::AudioNode as AudioNode>::Configuration>,
        fx_chain_config: Option<FX::Configuration>,
        dst_node_id: NodeID,
        call_update_when_done: bool,
        cx: &mut FirewheelContext,
    ) -> Result<Self, ModifyNodePoolError> {
        let dst_node_in_channels = NonZeroChannelCount::new(
            cx.node_channel_config(dst_node_id)
                .ok_or(ModifyNodePoolError::DstNodeNotFound(dst_node_id))?
                .num_inputs
                .get(),
        )
        .ok_or(ModifyNodePoolError::DstNodeNoInputs(dst_node_id))?;

        let mut workers: Vec<Worker<N, FX>> = Vec::with_capacity(num_workers.get());

        cx.try_modify_graph(|cx| -> Result<(), ModifyNodePoolError> {
            let fx_chain_config = fx_chain_config.unwrap_or_default();

            for _ in 0..num_workers.get() {
                let first_node_id = cx.add_node(first_node.clone(), first_node_config.clone())?;
                let first_node_out_channels = NonZeroChannelCount::new(
                    cx.node_channel_config(first_node_id)
                        .unwrap()
                        .num_outputs
                        .get(),
                )
                .ok_or(ModifyNodePoolError::FirstNodeNoOutput)?;

                let io = FxChainIo {
                    first_node_id,
                    first_node_out_channels,
                    dst_node_id,
                    dst_node_in_channels,
                };
                let mut fx_chain = FX::default();
                let node_ids = fx_chain.construct_and_connect(&fx_chain_config, &io, cx)?;

                workers.push(Worker {
                    first_node_params: first_node.clone(),
                    first_node_id,
                    additional_state: Default::default(),
                    fx_state: FxChainState { fx_chain, node_ids },
                    assigned_worker_id: None,
                });
            }

            if call_update_when_done {
                cx.update()?;
            }

            Ok(())
        })?;

        Ok(Self {
            workers,
            worker_ids: Arena::with_capacity(num_workers.get()),
            num_active_workers: 0,
            dst_node_id,
        })
    }

    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    /// Queue new work to play a sequence.
    ///
    /// * `params` - The parameters of the first node.
    /// * `time` - The instant these new parameters should take effect. If this
    ///   is `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    /// * `steal` - If this is `true`, then if there are no more workers left in
    ///   in the pool, the oldest one will be stopped and replaced with this new
    ///   one. If this is `false`, then an error will be returned if no more workers
    ///   are left.
    /// * `cx` - The Firewheel context.
    /// * `first_node` - A closure to send additional events to the first node, such
    ///   as setting the sample resource.
    /// * `fx_chain` - A closure to send events to the fx chain in this worker instance.
    ///
    /// This will return an error if `params.playback == PlaybackState::Stop`.
    pub fn new_worker(
        &mut self,
        params: &N::AudioNode,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        steal: bool,
        cx: &mut FirewheelContext,
        first_node: impl FnOnce(&mut ContextQueue),
        fx_chain: impl FnOnce(&mut FxChainState<FX>, &mut FirewheelContext),
    ) -> Result<NewWorkerResult, NewWorkerError> {
        if N::params_stopped(params) {
            return Err(NewWorkerError::ParameterStateIsStop);
        }

        if !steal && self.num_active_workers == self.workers.len() {
            return Err(NewWorkerError::NoMoreWorkers);
        }

        let mut idx = 0;
        let mut max_score = 0;
        for (i, worker) in self.workers.iter_mut().enumerate() {
            if worker.assigned_worker_id.is_none() {
                idx = i;
                break;
            }

            let score = N::worker_score(
                &worker.first_node_params,
                worker.first_node_id,
                &mut worker.additional_state,
                cx,
            )
            .unwrap();

            if score == u64::MAX {
                idx = i;
                break;
            }

            if score > max_score {
                max_score = score;
                idx = i;
            }
        }

        let worker_id = WorkerID(self.worker_ids.insert(idx));

        let worker = &mut self.workers[idx];

        let old_worker_id = worker.assigned_worker_id.take();
        let was_playing_sequence = if let Some(old_worker_id) = old_worker_id {
            self.worker_ids.remove(old_worker_id.0);

            !(N::params_stopped(params)
                || N::node_is_stopped(
                    worker.first_node_id,
                    &worker.first_node_params,
                    &mut worker.additional_state,
                    cx,
                )
                .unwrap())
        } else {
            false
        };

        worker.assigned_worker_id = Some(worker_id);
        self.num_active_workers += 1;

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(
            &worker.first_node_params,
            params,
            &mut worker.additional_state,
            &mut event_queue,
        );

        (first_node)(&mut event_queue);

        worker.first_node_params = params.clone();

        N::mark_playing(
            worker.first_node_id,
            &worker.first_node_params,
            &mut worker.additional_state,
            cx,
        )
        .unwrap();

        (fx_chain)(&mut worker.fx_state, cx);

        Ok(NewWorkerResult {
            worker_id,
            old_worker_id,
            first_node_id: worker.first_node_id,
            was_playing_sequence,
        })
    }

    /// Sync the parameters for the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `params` - The new parameter state to sync
    /// * `time` - The instant these new parameters should take effect. If this
    ///   is `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    /// * `cx` - The Firewheel context
    /// * `first_node` - A closure to send additional events to the first node, such
    ///   as setting the sample resource.
    /// * `fx_chain` - A closure to send events to the fx chain in this worker instance.
    ///
    /// If the parameters signify that the sequence is stopped, then this worker
    /// will be removed and the `worker_id` will be invalidated.
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn sync_worker_params(
        &mut self,
        worker_id: WorkerID,
        params: &N::AudioNode,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
        first_node: impl FnOnce(&mut ContextQueue),
        fx_chain: impl FnOnce(&mut FxChainState<FX>, &mut FirewheelContext),
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(
            &worker.first_node_params,
            params,
            &mut worker.additional_state,
            &mut event_queue,
        );

        (first_node)(&mut event_queue);

        worker.first_node_params = params.clone();

        (fx_chain)(&mut worker.fx_state, cx);

        if N::params_stopped(params) {
            self.worker_ids.remove(worker_id.0);
            worker.assigned_worker_id = None;
            self.num_active_workers -= 1;
        }

        true
    }

    /// Modify the list of nodes and connections in each FX chain instance.
    ///
    /// * `new_dst_node_id` - The ID of the new node that the last effect in each
    ///   fx chain instance should connect to. Set to `None` to use the previously
    ///   set destination node.
    /// * `call_update_when_done` - If `true`, then the [`FirewheelContext::update()`] will
    ///   be called after all new nodes have been added. This can be used to ensure that all
    ///   nodes activate without errors before committing the changes to the audio graph.
    /// * `cx` - The Firewheel context
    /// * `f` - A closure that is called on each FX chain instance. If nodes have
    ///   been added or removed, then the third argument `&mut Vec<NodeID>` must
    ///   be modified with the new list of node IDs.
    ///
    /// If an error is returned, then the audio graph has not been modified.
    pub fn modify_fx_chain(
        &mut self,
        new_dst_node_id: Option<NodeID>,
        call_update_when_done: bool,
        cx: &mut FirewheelContext,
        mut f: impl FnMut(&FxChainIo, &mut FX, &mut Vec<NodeID>) -> Result<(), ModifyNodePoolError>,
    ) -> Result<(), ModifyNodePoolError> {
        let dst_node_id = new_dst_node_id.unwrap_or(self.dst_node_id);
        let dst_node_in_channels = NonZeroChannelCount::new(
            cx.node_channel_config(dst_node_id)
                .ok_or(ModifyNodePoolError::DstNodeNotFound(dst_node_id))?
                .num_inputs
                .get(),
        )
        .ok_or(ModifyNodePoolError::DstNodeNoInputs(dst_node_id))?;

        cx.try_modify_graph(|cx| -> Result<(), ModifyNodePoolError> {
            for worker in self.workers.iter_mut() {
                let io = FxChainIo {
                    first_node_id: worker.first_node_id,
                    first_node_out_channels: NonZeroChannelCount::new(
                        cx.node_channel_config(worker.first_node_id)
                            .unwrap()
                            .num_outputs
                            .get(),
                    )
                    .unwrap(),
                    dst_node_id,
                    dst_node_in_channels,
                };

                (f)(
                    &io,
                    &mut worker.fx_state.fx_chain,
                    &mut worker.fx_state.node_ids,
                )?;
            }

            if call_update_when_done {
                cx.update()?;
            }

            Ok(())
        })
    }

    /// Pause the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the pause should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    /// * `cx` - The Firewheel context
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn pause(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::pause(&mut new_params, &mut worker.additional_state);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(
            &worker.first_node_params,
            &new_params,
            &mut worker.additional_state,
            &mut event_queue,
        );

        true
    }

    /// Resume the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the resume should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    /// * `cx` - The Firewheel context
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn resume(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::resume(&mut new_params, &mut worker.additional_state);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(
            &worker.first_node_params,
            &new_params,
            &mut worker.additional_state,
            &mut event_queue,
        );

        true
    }

    /// Stop the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the stop should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    /// * `cx` - The Firewheel context
    ///
    /// This will remove the worker and invalidate the given `worker_id`.
    ///
    /// Returns `true` if a worker with the given ID exists and was stopped.
    pub fn stop(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::stop(&mut new_params, &mut worker.additional_state);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(
            &worker.first_node_params,
            &new_params,
            &mut worker.additional_state,
            &mut event_queue,
        );

        self.worker_ids.remove(worker_id.0);
        worker.assigned_worker_id = None;
        self.num_active_workers -= 1;

        true
    }

    /// Pause all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    pub fn pause_all(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::pause(&mut new_params, &mut worker.additional_state);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(
                    &worker.first_node_params,
                    &new_params,
                    &mut worker.additional_state,
                    &mut event_queue,
                );
            }
        }
    }

    /// Resume all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    pub fn resume_all(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::resume(&mut new_params, &mut worker.additional_state);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(
                    &worker.first_node_params,
                    &new_params,
                    &mut worker.additional_state,
                    &mut event_queue,
                );
            }
        }
    }

    /// Stop all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    ///   `None`, then the parameters will take effect as soon as the node receives
    ///   the event.
    pub fn stop_all(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelContext,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::stop(&mut new_params, &mut worker.additional_state);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(
                    &worker.first_node_params,
                    &new_params,
                    &mut worker.additional_state,
                    &mut event_queue,
                );

                worker.assigned_worker_id = None;
            }
        }

        self.worker_ids.clear();
        self.num_active_workers = 0;
    }

    /// Get the first node parameters of the given worker.
    pub fn first_node(&self, worker_id: WorkerID) -> Option<&N::AudioNode> {
        self.worker_ids
            .get(worker_id.0)
            .map(|idx| &self.workers[*idx].first_node_params)
    }

    /// Get an immutable reference to the state of the first node of the given worker.
    pub fn first_node_state<'a, T: 'static>(
        &self,
        worker_id: WorkerID,
        cx: &'a FirewheelContext,
    ) -> Option<&'a T> {
        self.worker_ids
            .get(worker_id.0)
            .and_then(|idx| cx.node_state::<T>(self.workers[*idx].first_node_id))
    }

    /// Get a mutable reference to the state of the first node of the given worker.
    pub fn first_node_state_mut<'a, T: 'static>(
        &self,
        worker_id: WorkerID,
        cx: &'a mut FirewheelContext,
    ) -> Option<&'a mut T> {
        self.worker_ids
            .get(worker_id.0)
            .and_then(|idx| cx.node_state_mut::<T>(self.workers[*idx].first_node_id))
    }

    pub fn fx_chain(&self, worker_id: WorkerID) -> Option<&FxChainState<FX>> {
        self.worker_ids
            .get(worker_id.0)
            .map(|idx| &self.workers[*idx].fx_state)
    }

    pub fn fx_chain_mut(&mut self, worker_id: WorkerID) -> Option<&mut FxChainState<FX>> {
        self.worker_ids
            .get(worker_id.0)
            .map(|idx| &mut self.workers[*idx].fx_state)
    }

    /// The ID of the node that all fx chain outputs are connected to.
    pub fn dst_node_id(&self) -> NodeID {
        self.dst_node_id
    }

    /// Returns `true` if the sequence has either not started playing yet or has finished
    /// playing.
    pub fn has_stopped(&mut self, worker_id: WorkerID, cx: &mut FirewheelContext) -> bool {
        self.worker_ids
            .get(worker_id.0)
            .map(|idx| {
                let worker = &mut self.workers[*idx];
                N::node_is_stopped(
                    worker.first_node_id,
                    &worker.first_node_params,
                    &mut worker.additional_state,
                    cx,
                )
                .unwrap()
            })
            .unwrap_or(true)
    }

    /// Poll for the current number of active workers, and return a list of
    /// workers which have finished playing.
    ///
    /// Calling this method is optional.
    pub fn poll(&mut self, cx: &mut FirewheelContext) -> PollResult {
        self.num_active_workers = 0;
        let mut finished_workers = SmallVec::new();

        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                if N::node_is_stopped(
                    worker.first_node_id,
                    &worker.first_node_params,
                    &mut worker.additional_state,
                    cx,
                )
                .unwrap()
                {
                    let id = worker.assigned_worker_id.take().unwrap();
                    self.worker_ids.remove(id.0);
                    finished_workers.push(id);
                } else {
                    self.num_active_workers += 1;
                }
            }
        }

        PollResult { finished_workers }
    }

    /// The total number of active workers.
    pub fn num_active_workers(&self) -> usize {
        self.num_active_workers
    }

    /// Consume this audio node pool and remove all of its nodes from the audio graph.
    ///
    /// Returns a list of all node IDs and edges that were removed from the graph.
    pub fn remove_all_nodes(mut self, cx: &mut FirewheelContext) -> (Vec<NodeID>, Vec<Edge>) {
        let mut removed_nodes = Vec::new();
        let mut removed_edges = Vec::new();

        for worker in self.workers.drain(..) {
            worker.remove_nodes(cx, &mut removed_nodes, &mut removed_edges);
        }

        (removed_nodes, removed_edges)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    /// The worker IDs which have finished playing. These IDs are now
    /// invalidated.
    pub finished_workers: SmallVec<[WorkerID; 4]>,
}

/// The result of calling [`AudioNodePool::new_worker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkerResult {
    /// The new ID of the worker assigned to play this sequence.
    pub worker_id: WorkerID,

    /// The ID that was previously assigned to this worker.
    pub old_worker_id: Option<WorkerID>,

    /// The ID of the first node in this worker.
    pub first_node_id: NodeID,

    /// If this is `true`, then this worker was already playing a sequence, and that
    /// sequence has been stopped.
    pub was_playing_sequence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NewWorkerError {
    #[error(
        "Could not create new audio node pool worker: the given parameters signify a stopped sequence"
    )]
    ParameterStateIsStop,
    #[error("Could not create new audio node pool worker: the worker pool is full")]
    NoMoreWorkers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    #[error("A node with ID {0:?} does not exist in this pool")]
    InvalidNodeID(NodeID),
}

/// An error occured while creating or modify a [`AudioNodePool`].
#[derive(Debug, thiserror::Error)]
pub enum ModifyNodePoolError {
    /// The destination node was not found in the audio graph.
    #[error("The destination node {0:?} was not found")]
    DstNodeNotFound(NodeID),
    /// The destination node node has no input ports.
    #[error("The destination node {0:?} has no input ports")]
    DstNodeNoInputs(NodeID),
    /// The first node has no output ports.
    #[error("The first node has no output ports")]
    FirstNodeNoOutput,
    /// An error occured while adding a new node to the graph.
    #[error("{0}")]
    NodeError(NodeError),
    /// An error occured while removing a node from the graph.
    #[error("{0}")]
    RemoveNodeError(#[from] RemoveNodeError),
    /// An error occured while adding a new edge to the graph.
    #[error("{0}")]
    AddEdgeError(#[from] AddEdgeError),
    /// An error occurred while updating a Firewheel context.
    #[error("{0}")]
    UpdateError(#[from] UpdateError),
    /// An error while trying to compile the graph, i.e. a
    /// cycle was detected.
    #[error("{0}")]
    CompileGraphError(#[from] CompileGraphError),
}

impl From<NodeError> for ModifyNodePoolError {
    fn from(e: NodeError) -> Self {
        Self::NodeError(e)
    }
}
