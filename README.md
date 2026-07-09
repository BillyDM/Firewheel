<div align="center"><img src="./assets/logo-512.png" width="64px" height="64px"/><h1>Firewheel</h1></div>

[![Documentation](https://docs.rs/firewheel/badge.svg)](https://docs.rs/firewheel)
[![Crates.io](https://img.shields.io/crates/v/firewheel.svg)](https://crates.io/crates/firewheel)
[![License](https://img.shields.io/crates/l/firewheel.svg)](https://github.com/BillyDM/firewheel/blob/main/LICENSE-APACHE)

An open source audio graph engine for games and other applications, written in Rust. It can be used as-is, or it can be used as a base for other higher-level audio engines.

## The Future of this Project

Firewheel is currently planned to be upstreamed into the [Bevy](https://bevy.org/) game engine where it will become the default audio engine. (The current implementation can be found in the [bevy_seedling](https://github.com/CorvusPrudens/bevy_seedling) repository.) The goal is to both make Bevy integration easier by avoiding circular dependencies, and to ease the maintanance burden of a project that has experienced more feature creep than what the author originally anticipated.

While development has shifted to focus more on the needs of Bevy, the core audio graph engine will still be available to use outside of Bevy without any other Bevy dependencies (except for the very lightweight [bevy_platform](https://crates.io/crates/bevy_platform) dependency).

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

## Non-features

While Firewheel aims to cover most use cases for games and other generic applications, it does *NOT* aim to be a complete DAW (digital audio workstation) engine. Not only would this greatly increase complexity, but the needs of game audio engines and DAW audio engines are in conflict. (See the design document for more details on why).

## Get Involved

Join the discussion in the [Bevy Discord Server](https://discord.gg/bevy) under the `working-groups -> Better Audio` channel!

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0), or
* MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.

[Design Document]: DESIGN_DOC.md
[CPAL]: https://github.com/RustAudio/cpal
[Symphonium]: https://codeberg.org/Meadowlark/symphonium
[CLAP]: https://cleveraudio.org/
