# Firewheel Design Document

## Overview

Both the Rust ecosystem and the libre game engine ecosystem as a whole are in need of a powerful, flexible, and libre audio engine for games. Firewheel aims to provide developers with a powerful and modular solution for constructing custom interactive audio experiences.

> #### Why the name "Firewheel"?
> The [firewheel](https://en.wikipedia.org/wiki/Gaillardia_pulchella) (aka "Indian Blanket", scientific name Gaillardia Pulchella) is a wildflower native to the Midwest USA. I just thought it was a cool looking flower with a cool name. :)

## Goals for First Release

* [x] Modular design that can be run on any backend that provides an audio stream.
    * [x] [CPAL] backend. This gives us support for Windows, Mac, Linux, Android, iOS, and WebAssembly.
* [x] Flexible audio graph engine (supports any directed, acyclic graph with support for both one-to-many and many-to-one connections)
* [x] Cycle detection for invalid audio graphs
* Key built-in nodes:
    * [x] volume (minimum value mutes)
    * [x] stereo panning
    * [x] stereo to mono
    * [x] decibel (peak) meter
    * [x] beep test (generates a sine wav for testing)
    * [x] stream writer (put raw audio samples into the graph from another thread)
    * [x] stream reader (read samples directly from the audio graph from another thread)
    * [x] sampler node
    * [x] simple spatial positioning (only the simplest implementation for first release)
* [x] Custom audio node API allowing for a plethora of 3rd party generators and effects
* [x] Silence optimizations (avoid processing if the audio buffer contains all zeros, useful when using "pools" of nodes where the majority of the time nodes are unused.)
* [x] Support for loading a wide variety of audio formats (using [Symphonium](https://github.com/MeadowlarkDAW/symphonium))
* [x] Fault tolerance for audio streams (The game shouldn't crash just because the player accidentally unplugged their headphones.)
* [x] Option to hard clip outputs at 0dB to help protect the system's speakers.
* [x] Properly respect realtime constraints (no mutexes!)
* [x] Windows, Mac, and Linux support 
* [x] Verify WebAssembly support (Note special considerations must be made about the design of the threading model.)

## Later Goals

* [x] Sequencing support for the sampler node
* [x] Doppler stretching (pitch shifting) on sampler node
* Extra built-in nodes:
    * [x] delay compensation
    * [x] convolution (user can load any impulse response they want to create effects like reverbs)
    * [x] filters (lowpass, highpass, bandpass)
    * [ ] echo
    * [ ] better spatial positioning with HRTF
    * [ ] equalizer
    * [ ] compressor
* [ ] Basic [CLAP] plugin hosting (non-WebAssembly only)
* [ ] C bindings

### * Notes on C bindings

Firewheel is a large library with a lot of rust-isms (enums, traits, generics, and macros). As such, the goal of the C bindings is NOT to provide access to every single part of the API. Instead, this is a rough overview of what the C bindings will consist of:

* Two backend options:
    * A "CPAL" option which allows you to enumerate available audio devices and to start/stop audio streams.
    * A "no backend" option where the user plugs in a firewheel processor object into their own custom audio stream.
* Optional functions for loading sample resources from memory or from a file path.
* Firewheel Context
    * Construct a Firewheel context with configurable options
    * Methods to get and configure clocks
    * Update method
    * Methods to add, remove, and connect nodes in the graph 
        * Some thought will need to be given on what the API to construct nodes should look like. We can simply just have a constructor function for each of the factory nodes. Though preferably there should also be a way to use construct third party nodes as well.
* Nodes
    * Sending updates to nodes will be purely event-driven (no data-driven diffing/patching system). This will be done by defining an update function for each parameter for each factory node. Though some thought will need to be given on what the API for third party nodes should look like.
    * Retrieving state will also be achieved using custom function for each of the factory nodes that have custom state.
* Node Pools
    * I haven't figured out what the API for node pools should look like yet.
* A custom node API. This will be similar to the API in Rust, except that the event types will be simplified to be simple parameter values with integer paramter IDs. There will also be no `ProcStore` API (instead users can just simply pass around raw pointers to resources they need to share across nodes).

## Maybe Goals

* [ ] A `SampleResource` with disk streaming support (using [creek](https://github.com/MeadowlarkDAW/creek))
* [ ] A `SampleResource` with network streaming support
* [ ] [RtAudio](https://github.com/thestk/rtaudio) backend
* [ ] [Interflow](https://github.com/SolarLiner/interflow) backend

## Non-Goals

While Firewheel is meant to cover nearly every use case for games and generic applications, it is not meant to be a complete DAW (digital audio workstation) engine. Not only would this greatly increase complexity, but the needs of game audio engine and DAW audio engine are in conflict*.

> \* This conflict arises in how state is expected to be synchronized between the user's state and the state of the processor. In a DAW, the state is tied to the state of the "transport", and the host is allowed to discard any user-generated parameter update events that conflict with this transport state (or vice versa). However, in a game engine and other generic applications, the user's state can dynamically change at any time. So to avoid the processor state from becoming desynchronized with the user's state, parameter update events are only allowed to come from a single source (the user), and are guaranteed to not be discarded by the engine.

* MIDI on the audio-graph level (It will still be possible to create a custom sampler/synthesizer nodes that read MIDI files as input.)
    * EDIT: There is now a similar way to achieve this using the `ProcStore`.
* Parameter events on the audio-graph level (as in you can't pass parameter update events from one node to another)
    * EDIT: There is now a similar way to achieve this using the `ProcStore`.
* Connecting to system MIDI devices (Although this feature could be added in the future if there is enough demand for it).
* Built-in synthesizer instruments (This can still be done with third-party nodes/CLAP plugins.)
* GUIs for hosted CLAP plugins.
* Multi-threaded audio graph processing (This would make the engine a lot more complicated, and it is probably overkill for games and generic applications.)
* VST, VST3, LV2, and AU plugin hosting

## Codebase Overview

* `firewheel-core` - Contains common types and utilities shared by Firewheel crates. It also houses the audio node API.
* `firewheel-cpal` - Contains the default [CPAL] backend.
* `firewheel-graph` - Contains the core audio graph engine.
* `firewheel-macros` - Contains various macros for diffing and patching parameters.
* `firewheel-nodes` - Contains the built-in factory nodes.
* `firewheel-pool` - Allows users to create pools of nodes that can be dynamically assigned work.

## Audio Node API

See [crates/firewheel-core/src/node.rs](crates/firewheel-core/src/node.rs)

## Backend API

Audio backends should have the following features:

* Retrieve a list of audio output and/or audio input devices so games can let the user choose which audio devices to use in the game's setting GUI.
* Spawn an audio stream with the chosen input/output devices (or `None` which specifies to use the default device).
    * If the device is not found, try falling back to the default audio device first before returning an error (if the user specified that they want to fall back).
    * If no default device is found, try falling back to a "dummy" audio device first before returning an error (if the user specified that they want to fall back).
* While the stream is running, the internal clock should be updated accordingly before calling `FirewheelProcessor::process()`. (See the `Clocks and Events` section below.)
* If an error occurs, notify the user of the error when they call the `update()` method. From there the user can decide how to respond to the error (try to reconnect, fallback to a different device, etc.)

## Engine Lifecycle

1. A context with an audio graph is initialized.
2. The context is "activated" using an audio stream given to it by the backend. A realtime-safe message channel is created, along with a processor (executor) that is sent to the audio stream. Then the audio graph is "compiled" into a schedule and sent to the executor over the message channel. If compiling fails, then the context will be deactivated again and return an error.
3. "Active" state:
    - The user periodically calls the `update` method on the context (i.e. once every frame). This method first flushes any events that are in the queue and sends them to the audio thread. (Flushing events as a group like this ensures that events that are expected to happen on the same process cycle don't happen on different process cycles.) Then this method checks for any changes in the graph, and compiles a new schedule if a change is detected. If there was an error compiling the graph, then the update method will return an error and a new schedule will not be created.
4. The context can become deactivated in one of two ways:
    * a. The user requests to deactivate the context. This is necessary, for example, when changing the audio io devices in the game's settings. Dropping the context will also automatically deactivate it first.
    * b. The audio stream is interrupted (i.e. the user unplugged the audio device). In this case, it is up to the developer/backend to decide how to respond (i.e. automatically try to activate again with a different device, or falling back to a "dummy" audio device).

## Clocks and Events

There are three clocks in the audio stream: the seconds clock, the sample clock, and the musical clock.

### Seconds Clock

This clock is recommended for most general use cases. It counts the total number of seconds (as an `f64` value) that have elapsed since the start of the audio stream. This value is read from the OS's native audio API where possible, so it is quite accurate and it correctly accounts for any output underflows that may occur.

Usage of the clock works like this:

1. Before sending an event to an audio node, the user calls `AudioGraph::clock_now()` to retrieve the current clock time.
2. For any event type that accepts an `EventDelay` parameter, the user will schedule the event like so: `EventDelay::DelayUntilSeconds(AudioGraph::clock_now() + desired_amount_of_delay)`.

### Sample clock

This works the same as `Seconds Clock`, except it simply counts the total number of samples that have been processed since the stream was started. The is very accurate, but it does not correctly account for any output underflows that may occur.

### Musical Clock

This clock is manually started, paused, resumed, and stopped by the user. It counts the number of musical beats (as an `f64` value) that have elapsed since the `MusicalTransport` was started. This clock is ideal for syncing events to a musical tempo. Though like the sample clock, it does not account for any output underflows that may occur. Instead, the user is expected to poll the current time of the clock from the context to keep their game in sync.

## Silence Optimizations

It is common to have a "pool of audio nodes" at the ready to accept work from a certain maximum number of concurrent audio instances in the game engine. However, this means that the majority of the time, most of these nodes will be unused which would lead to a lot of unnecessary processing.

To get around this, every audio buffer in the graph is marked with a "silence flag". Audio nodes can read `ProcInfo::in_silence_mask` to quickly check which input buffers contain silence. If all input buffers are silent, then the audio node can choose to skip processing.

Audio nodes which output audio also must notify the graph on which output channels should/do contain silence. See `ProcessStatus` in [node.rs](crates/firewheel-core/src/node.rs) for more details.

## Sampler

The sampler nodes are used to play back audio files (sound FX, music, etc.). Samplers can play back any resource which implements the `SampleResource` trait in [sample_resource.rs](crates/firewheel-core/src/sample_resource.rs). Using a trait like this gives the game engine control over how to load and store audio assets, i.e. by using a crate like [Symphonium](https://github.com/MeadowlarkDAW/symphonium).

## WebAssembly Considerations

Since WebAssembly (WASM) is one of the targets, special considerations must be made to make the engine and audio nodes work with it. These include (but are not limited to):

* No C or System Library Dependencies
    * Because of this, hosting [CLAP] plugins is not possible in WASM. So that feature will be disabled when compiling to that platform.
* No File I/O
    * Asset loading is out of scope of this project. Game engines themselves should be in charge of loading assets.
* Don't Spawn Threads
    * The audio backend (i.e. [CPAL]) should be in charge of spawning the audio thread.
    * While the [creek](https://github.com/MeadowlarkDAW/creek) crate requires threads, file operations aren't supported in WASM anyway, so this crate can just be disabled when compiling to WASM.
* Don't Block Threads

[CPAL]: https://github.com/RustAudio/cpal
[CLAP]: https://github.com/free-audio/clap