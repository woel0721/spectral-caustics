//! Unified integration test binary consolidating all end-to-end test suites.

mod beveled_prism;
mod config_render;
mod diagnostics_feature;
mod disk_area_light;
mod environment_scene;
mod forward_photons;
mod grid_medium;
mod grid_scattering;
mod material_transport;
mod medium_interface;
#[cfg(feature = "diagnostics")]
mod path_debug;
mod procedural_medium;
mod spectral_camera;
mod spectral_caustics;
mod sphere_caustics;
mod sphere_transport;
mod support;
mod vertical_prism;
mod volume;
