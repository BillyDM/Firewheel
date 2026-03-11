<div align="center"><img src="./assets/logo-512.png" width="64px" height="64px"/><h1>Firewheel</h1></div>

[![Documentation](https://docs.rs/firewheel/badge.svg)](https://docs.rs/firewheel)
[![Crates.io](https://img.shields.io/crates/v/firewheel.svg)](https://crates.io/crates/firewheel)
[![License](https://img.shields.io/crates/l/firewheel.svg)](https://github.com/BillyDM/firewheel/blob/main/LICENSE-APACHE)

A mid-level open source audio graph engine for games and other applications, written in Rust.

This crate can be used as-is or as a base for other higher-level audio engines. (Think of it like [wgpu](https://wgpu.rs/) but for audio).
NOTE: I disagree that this is like wgpu for audio. This is a step higher than wgpu, since it provides DSP infrastructure. This is more akin
to bevy_render, and cpal is more like wgpu in this analogy. ALSA/JACK/WASAPI/ASIO/CoreAudio, etc are equivalent to Vulkan/Metal/DirectX/OpenGL/WebGPU etc.

## Key Features

* Modular design that can be run on any backend that provides an audio stream
    * Included backends supporting Windows, Mac, Linux, Android, iOS, and WebAssembly
* Flexible audio graph engine (supports any directed, acyclic graph with support for both one-to-many and many-to-one connections)
* A suite of essential built-in audio nodes
* Custom audio node API allowing for a plethora of 3rd party generators and effects
* An optional data-driven parameter API that is friendly to ECS's (entity component systems).
* Silence optimizations (avoid processing if the audio buffer contains all zeros, useful when using "pools" of nodes where the majority of the time nodes are unused)
* Support for loading a wide variety of audio files using [Symphonium]
* Fault tolerance for audio streams (The game shouldn't stop or crash just because the player accidentally unplugged their headphones.)
* Properly respects realtime constraints (no mutexes!)
* `no_std` compatibility (some features require the standard library)
* (TODO) Basic [CLAP] plugin hosting (non-WASM only), allowing for more open source and proprietary 3rd party effects and synths
* (TODO) Bindings for C, and (possibly) C++ and C#

## Non-features

While Firewheel is meant to cover nearly every use case for games and other applications, it is not meant to be a complete DAW (digital audio workstation) engine. Not only would this greatly increase complexity, but the needs of game audio engines and DAW audio engines are in conflict. (See the design document for more details on why).

## Get Involved

Join the discussion in the [Firewheel Discord Server](https://discord.gg/rKzZpjGCGs) or in the [Bevy Discord Server](https://discord.gg/bevy) under the `working-groups -> Better Audio` channel!

If you are interested in contributing code, first read the [Design Document] and then visit the [Project Board](https://github.com/users/BillyDM/projects/1).

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0), or
* MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.

[Design Document]: DESIGN_DOC.md
[CPAL]: https://github.com/RustAudio/cpal
[Symphonium]: https://codeberg.org/Meadowlark/symphonium
[CLAP]: https://cleveraudio.org/
