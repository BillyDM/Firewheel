/// A trait defining all functions a generic filter needs to support
pub trait Filter {
    /// Filter memory below this value should be considered silent.
    /// Set to the same value Reason's Rack Extensions should use, so probably a good default.
    const SILENT_THRESHOLD: f32 = 2.0e-8f32;

    /// The type of coefficients needed for the filter to process samples
    type Coeffs;

    /// Resets the filter's memory
    fn reset(&mut self);

    /// Processes a single sample (should be forced to be inlined)
    fn process(&mut self, x: f32, coeffs: &Self::Coeffs) -> f32;

    /// Checks whether the filter is silent, i.e. whether all the memory is <= Self::SILENT_THRESHOLD
    fn is_silent(&self) -> bool;
}
