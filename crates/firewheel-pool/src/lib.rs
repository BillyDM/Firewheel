#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use firewheel_core::{
    channel_config::NonZeroChannelCount,
    node::{AudioNode, NodeID},
};
use firewheel_graph::{backend::AudioBackend, ContextQueue, FirewheelCtx};
use smallvec::SmallVec;
use thunderdome::Arena;

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

#[cfg(feature = "sampler")]
mod sampler;
#[cfg(feature = "sampler")]
pub use sampler::SamplerPool;

mod volume_pan;
pub use volume_pan::VolumePanChain;

#[cfg(feature = "spatial_basic")]
mod spatial_basic;
#[cfg(feature = "spatial_basic")]
pub use spatial_basic::SpatialBasicChain;

#[cfg(feature = "sampler")]
pub type SamplerPoolVolumePan = AudioNodePool<SamplerPool, VolumePanChain>;
#[cfg(all(feature = "sampler", feature = "spatial_basic"))]
pub type SamplerPoolSpatialBasic = AudioNodePool<SamplerPool, SpatialBasicChain>;

/// A trait describing an "FX chain" for use in an [`AudioNodePool`].
pub trait FxChain: Default {
    /// Construct the nodes in the FX chain and connect them, returning a list of the
    /// new node ids.
    ///
    /// * `first_node_id` - The ID of the first node in this fx chain instance.
    /// * `first_node_num_out_channels` - The number of output channels in the first node.
    /// * `dst_node_id` - The ID of the node that the last node in this FX chain should
    /// connect to.
    /// * `dst_num_channels` - The number of input channels on `dst_node_id`.
    /// * `cx` - The firewheel context.
    fn construct_and_connect<B: AudioBackend>(
        &mut self,
        first_node_id: NodeID,
        first_node_num_out_channels: NonZeroChannelCount,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelCtx<B>,
    ) -> Vec<NodeID>;
}

struct Worker<N: PoolableNode, FX: FxChain> {
    first_node_params: N::AudioNode,
    first_node_id: NodeID,

    fx_state: FxChainState<FX>,

    assigned_worker_id: Option<WorkerID>,
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

    /// Return the number of output channels for the given configuration.
    fn num_output_channels(
        config: Option<&<Self::AudioNode as AudioNode>::Configuration>,
    ) -> NonZeroChannelCount;

    /// Return `true` if the given parameters signify that the sequence is stopped,
    /// `false` otherwise.
    fn params_stopped(params: &Self::AudioNode) -> bool;
    /// Return `true` if the node state of the given node is stopped.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn node_is_stopped<B: AudioBackend>(
        node_id: NodeID,
        cx: &FirewheelCtx<B>,
    ) -> Result<bool, PoolError>;

    /// Return a score of how ready this node is to accept new work.
    ///
    /// The worker with the highest worker score will be chosen for the new work.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn worker_score<B: AudioBackend>(
        params: &Self::AudioNode,
        node_id: NodeID,
        cx: &mut FirewheelCtx<B>,
    ) -> Result<u64, PoolError>;

    /// Diff the new parameters and push the changes into the event queue.
    fn diff<B: AudioBackend>(
        baseline: &Self::AudioNode,
        new: &Self::AudioNode,
        event_queue: &mut ContextQueue<B>,
    );

    /// Notify the node state that a sequence is playing.
    ///
    /// This is used to account for the delay between sending an event to the node
    /// and the node receiving the event.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn mark_playing<B: AudioBackend>(
        node_id: NodeID,
        cx: &mut FirewheelCtx<B>,
    ) -> Result<(), PoolError>;

    /// Pause the sequence in the node parameters
    fn pause(params: &mut Self::AudioNode);
    /// Resume the sequence in the node parameters
    fn resume(params: &mut Self::AudioNode);
    /// Stop the sequence in the node parameters
    fn stop(params: &mut Self::AudioNode);
}

/// A pool of audio node chains that can dynamically be assigned work.
pub struct AudioNodePool<N: PoolableNode, FX: FxChain> {
    workers: Vec<Worker<N, FX>>,
    worker_ids: Arena<usize>,
    num_active_workers: usize,
}

impl<N: PoolableNode, FX: FxChain> AudioNodePool<N, FX>
where
    <N::AudioNode as AudioNode>::Configuration: Clone,
{
    /// Construct a new sampler pool.
    ///
    /// * `num_workers` - The total number of workers that can work in parallel. More workers
    /// will allow more samples to be played concurrently, but will also increase processing
    /// overhead. A value of `16` is a good place to start.
    /// * `first_node` - The state of the first node in each FX chain instance.
    /// * `first_node_config` - The configuration of the first node in each FX chain instance.
    /// * `first_node_num_out_channels` - The number of output channels in the first node.
    /// * `dst_node_id` - The ID of the node that the last effect in each fx chain instance
    /// will connect to.
    /// * `dst_num_channels` - The number of input channels in `dst_node_id`.
    /// * `cx` - The firewheel context.
    pub fn new<B: AudioBackend>(
        num_workers: usize,
        first_node: N::AudioNode,
        first_node_config: Option<<N::AudioNode as AudioNode>::Configuration>,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelCtx<B>,
    ) -> Self {
        assert_ne!(num_workers, 0);

        let first_node_num_out_channels = N::num_output_channels(first_node_config.as_ref());

        Self {
            workers: (0..num_workers)
                .map(|_| {
                    let first_node_id = cx.add_node(first_node.clone(), first_node_config.clone());

                    let mut fx_chain = FX::default();

                    let fx_ids = fx_chain.construct_and_connect(
                        first_node_id,
                        first_node_num_out_channels,
                        dst_node_id,
                        dst_num_channels,
                        cx,
                    );

                    Worker {
                        first_node_params: first_node.clone(),
                        first_node_id,

                        fx_state: FxChainState {
                            fx_chain,
                            node_ids: fx_ids,
                        },

                        assigned_worker_id: None,
                    }
                })
                .collect(),
            worker_ids: Arena::with_capacity(num_workers),
            num_active_workers: 0,
        }
    }

    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    /// Queue a new work to play a sequence.
    ///
    /// * `params` - The parameters of the sequence to play.
    /// * `time` - The instant these new parameters should take effect. If this
    /// is `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    /// * `steal` - If this is `true`, then if there are no more workers left in
    /// in the pool, the oldest one will be stopped and replaced with this new
    /// one. If this is `false`, then an error will be returned if no more workers
    /// are left.
    /// * `cx` - The Firewheel context.
    /// * `fx_chain` - A closure to add additional nodes to this worker instance.
    ///
    /// This will return an error if `params.playback == PlaybackState::Stop`.
    pub fn new_worker<B: AudioBackend>(
        &mut self,
        params: &N::AudioNode,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        steal: bool,
        cx: &mut FirewheelCtx<B>,
        fx_chain: impl FnOnce(&mut FxChainState<FX>, &mut FirewheelCtx<B>),
    ) -> Result<NewWorkerResult, NewWorkerError> {
        if N::params_stopped(params) {
            return Err(NewWorkerError::ParameterStateIsStop);
        }

        if !steal && self.num_active_workers == self.workers.len() {
            return Err(NewWorkerError::NoMoreWorkers);
        }

        let mut idx = 0;
        let mut max_score = 0;
        for (i, worker) in self.workers.iter().enumerate() {
            if worker.assigned_worker_id.is_none() {
                idx = i;
                break;
            }

            let score =
                N::worker_score(&worker.first_node_params, worker.first_node_id, cx).unwrap();

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

            !(N::params_stopped(params) || N::node_is_stopped(worker.first_node_id, cx).unwrap())
        } else {
            false
        };

        worker.assigned_worker_id = Some(worker_id);
        self.num_active_workers += 1;

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(&worker.first_node_params, params, &mut event_queue);

        worker.first_node_params = params.clone();

        N::mark_playing(worker.first_node_id, cx).unwrap();

        (fx_chain)(&mut worker.fx_state, cx);

        Ok(NewWorkerResult {
            worker_id,
            old_worker_id,
            was_playing_sequence,
        })
    }

    /// Sync the parameters for the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `params` - The new parameter state to sync
    /// * `time` - The instant these new parameters should take effect. If this
    /// is `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    /// * `cx` - The Firewheel context
    ///
    /// If the parameters signify that the seuquence is stopped, then this worker
    /// will be removed and the `worker_id` will be invalidated.
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn sync_worker_params<B: AudioBackend>(
        &mut self,
        worker_id: WorkerID,
        params: &N::AudioNode,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(&worker.first_node_params, params, &mut event_queue);

        worker.first_node_params = params.clone();

        if N::params_stopped(params) {
            self.worker_ids.remove(worker_id.0);
            worker.assigned_worker_id = None;
            self.num_active_workers -= 1;
        }

        true
    }

    /// Pause the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the pause should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    /// * `cx` - The Firewheel context
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn pause<B: AudioBackend>(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::pause(&mut new_params);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(&worker.first_node_params, &new_params, &mut event_queue);

        true
    }

    /// Resume the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the resume should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    /// * `cx` - The Firewheel context
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn resume<B: AudioBackend>(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::resume(&mut new_params);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(&worker.first_node_params, &new_params, &mut event_queue);

        true
    }

    /// Stop the given worker.
    ///
    /// * `worker_id` - The ID of the worker
    /// * `time` - The instant that the stop should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    /// * `cx` - The Firewheel context
    ///
    /// This will remove the worker and invalidate the given `worker_id`.
    ///
    /// Returns `true` if a worker with the given ID exists and was stopped.
    pub fn stop<B: AudioBackend>(
        &mut self,
        worker_id: WorkerID,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut new_params = worker.first_node_params.clone();
        N::stop(&mut new_params);

        #[cfg(not(feature = "scheduled_events"))]
        let mut event_queue = cx.event_queue(worker.first_node_id);
        #[cfg(feature = "scheduled_events")]
        let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

        N::diff(&worker.first_node_params, &new_params, &mut event_queue);

        self.worker_ids.remove(worker_id.0);
        worker.assigned_worker_id = None;
        self.num_active_workers -= 1;

        true
    }

    /// Pause all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    pub fn pause_all<B: AudioBackend>(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::pause(&mut new_params);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(&worker.first_node_params, &new_params, &mut event_queue);
            }
        }
    }

    /// Resume all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    pub fn resume_all<B: AudioBackend>(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::resume(&mut new_params);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(&worker.first_node_params, &new_params, &mut event_queue);
            }
        }
    }

    /// Stop all workers.
    ///
    /// * `time` - The instant that the stop should take effect. If this is
    /// `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    pub fn stop_all<B: AudioBackend>(
        &mut self,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        cx: &mut FirewheelCtx<B>,
    ) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                let mut new_params = worker.first_node_params.clone();
                N::stop(&mut new_params);

                #[cfg(not(feature = "scheduled_events"))]
                let mut event_queue = cx.event_queue(worker.first_node_id);
                #[cfg(feature = "scheduled_events")]
                let mut event_queue = cx.event_queue_scheduled(worker.first_node_id, time);

                N::diff(&worker.first_node_params, &new_params, &mut event_queue);

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
    pub fn first_node_state<'a, T: 'static, B: AudioBackend>(
        &self,
        worker_id: WorkerID,
        cx: &'a FirewheelCtx<B>,
    ) -> Option<&'a T> {
        self.worker_ids
            .get(worker_id.0)
            .and_then(|idx| cx.node_state::<T>(self.workers[*idx].first_node_id))
    }

    /// Get a mutable reference to the state of the first node of the given worker.
    pub fn first_node_state_mut<'a, T: 'static, B: AudioBackend>(
        &self,
        worker_id: WorkerID,
        cx: &'a mut FirewheelCtx<B>,
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

    /// Returns `true` if the sequence has either not started playing yet or has finished
    /// playing.
    pub fn has_stopped<B: AudioBackend>(&self, worker_id: WorkerID, cx: &FirewheelCtx<B>) -> bool {
        self.worker_ids
            .get(worker_id.0)
            .map(|idx| N::node_is_stopped(self.workers[*idx].first_node_id, cx).unwrap())
            .unwrap_or(true)
    }

    /// Poll for the current number of active workers, and return a list of
    /// workers which have finished playing.
    ///
    /// Calling this method is optional.
    pub fn poll<B: AudioBackend>(&mut self, cx: &FirewheelCtx<B>) -> PollResult {
        self.num_active_workers = 0;
        let mut finished_workers = SmallVec::new();

        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                if N::node_is_stopped(worker.first_node_id, cx).unwrap() {
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

    /// If this is `true`, then this worker was already playing a sequence, and that
    /// sequence has been stopped.
    pub was_playing_sequence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NewWorkerError {
    #[error("Could not create new audio node pool worker: the given parameters signify a stopped sequence")]
    ParameterStateIsStop,
    #[error("Could not create new audio node pool worker: the worker pool is full")]
    NoMoreWorkers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    #[error("A node with ID {0:?} does not exist in this pool")]
    InvalidNodeID(NodeID),
}
