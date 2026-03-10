/// A simple two-dimensional vector type.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A simple three-dimensional vector type.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

impl From<[f32; 2]> for Vec2 {
    fn from(value: [f32; 2]) -> Self {
        Self::new(value[0], value[1])
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(value: [f32; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }
}

impl From<(f32, f32)> for Vec2 {
    fn from(value: (f32, f32)) -> Self {
        Self::new(value.0, value.1)
    }
}

impl From<(f32, f32, f32)> for Vec3 {
    fn from(value: (f32, f32, f32)) -> Self {
        Self::new(value.0, value.1, value.2)
    }
}

impl From<Vec2> for [f32; 2] {
    fn from(value: Vec2) -> Self {
        [value.x, value.y]
    }
}

impl From<Vec3> for [f32; 3] {
    fn from(value: Vec3) -> Self {
        [value.x, value.y, value.z]
    }
}

impl From<Vec2> for (f32, f32) {
    fn from(value: Vec2) -> Self {
        (value.x, value.y)
    }
}

impl From<Vec3> for (f32, f32, f32) {
    fn from(value: Vec3) -> Self {
        (value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-29")]
impl From<glam_29::Vec2> for Vec2 {
    fn from(value: glam_29::Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-29")]
impl From<glam_29::Vec3> for Vec3 {
    fn from(value: glam_29::Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-29")]
impl From<Vec2> for glam_29::Vec2 {
    fn from(value: Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-29")]
impl From<Vec3> for glam_29::Vec3 {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-30")]
impl From<glam_30::Vec2> for Vec2 {
    fn from(value: glam_30::Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-30")]
impl From<glam_30::Vec3> for Vec3 {
    fn from(value: glam_30::Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-30")]
impl From<Vec2> for glam_30::Vec2 {
    fn from(value: Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-30")]
impl From<Vec3> for glam_30::Vec3 {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-31")]
impl From<glam_31::Vec2> for Vec2 {
    fn from(value: glam_31::Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-31")]
impl From<glam_31::Vec3> for Vec3 {
    fn from(value: glam_31::Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-31")]
impl From<Vec2> for glam_31::Vec2 {
    fn from(value: Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-31")]
impl From<Vec3> for glam_31::Vec3 {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-32")]
impl From<glam_32::Vec2> for Vec2 {
    fn from(value: glam_32::Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-32")]
impl From<glam_32::Vec3> for Vec3 {
    fn from(value: glam_32::Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[cfg(feature = "glam-32")]
impl From<Vec2> for glam_32::Vec2 {
    fn from(value: Vec2) -> Self {
        Self::new(value.x, value.y)
    }
}

#[cfg(feature = "glam-32")]
impl From<Vec3> for glam_32::Vec3 {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}