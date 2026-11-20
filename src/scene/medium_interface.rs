//! Participating-medium sides of a single closed dielectric. Regions belong to one
//! scene with stable object indices for the lifetime of a transport path.
use super::{Scene, SceneObject};
use crate::{
    geometry::{hit::Hit, object::PointLocation},
    material::ScatterType,
    medium::Medium,
};
use nalgebra::Vector3;

/// The exterior medium is owned by Scene. An empty interior denotes vacuum.
pub struct MediumInterface {
    pub interior: Option<Box<dyn Medium>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediumRegion {
    Exterior,
    /// Index of the object carrying the boundary, independent of the optical IOR.
    Interior(usize),
}

impl Scene {
    fn medium_boundary(&self) -> Option<(usize, &SceneObject)> {
        self.objects
            .iter()
            .enumerate()
            .find(|(_, object)| object.medium_interface.is_some())
    }

    pub(crate) fn has_medium_interface(&self) -> bool {
        self.medium_boundary().is_some()
    }

    pub(crate) fn has_wavelength_dependent_medium(&self) -> bool {
        self.medium
            .as_deref()
            .into_iter()
            .chain(
                self.objects
                    .iter()
                    .filter_map(|object| object.medium_interface.as_ref()?.interior.as_deref()),
            )
            .any(Medium::is_wavelength_dependent)
    }

    pub fn bind_medium_interface(
        &mut self,
        object_id: usize,
        interface: MediumInterface,
    ) -> Result<(), &'static str> {
        if self.has_medium_interface() {
            return Err("only one medium interface is supported");
        }
        let object = self
            .objects
            .get(object_id)
            .ok_or("invalid boundary object")?;
        let material = self.material(object.material_id());
        if !material.has_delta()
            || material.has_non_delta()
            || !material
                .ior()
                .is_some_and(|ior| ior.is_finite() && ior > 0.0)
        {
            return Err("medium interface requires an ideal dielectric");
        }
        if object.geometry.closed_volume().is_none() {
            return Err("medium interface requires closed-volume classification");
        }
        self.objects[object_id].medium_interface = Some(interface);
        Ok(())
    }

    pub fn active_medium(&self, region: MediumRegion) -> Result<Option<&dyn Medium>, &'static str> {
        match region {
            MediumRegion::Exterior => Ok(self.medium.as_deref()),
            MediumRegion::Interior(id) => {
                let interface = self
                    .objects
                    .get(id)
                    .and_then(|o| o.medium_interface.as_ref())
                    .ok_or("interior region requires its bound object")?;
                Ok(interface.interior.as_deref())
            }
        }
    }

    /// Classify a fresh launch before transporting its first segment. At a surface
    /// the caller must specify the departure side. A known side elsewhere must agree.
    pub fn initial_medium_region(
        &self,
        origin: Vector3<f64>,
        departure: Option<MediumRegion>,
    ) -> Result<MediumRegion, &'static str> {
        if !origin.iter().all(|v| v.is_finite()) {
            return Err("non-finite launch point");
        }
        let mut region = MediumRegion::Exterior;
        if let Some((id, object)) = self.medium_boundary() {
            let solid = object
                .geometry
                .closed_volume()
                .ok_or("bound geometry is no longer closed")?;
            region = match solid.classify_point(origin)? {
                PointLocation::Inside => MediumRegion::Interior(id),
                PointLocation::Outside => MediumRegion::Exterior,
                PointLocation::OnBoundary => {
                    departure.ok_or("boundary launch requires a departure side")?
                }
            };
            if matches!(region, MediumRegion::Interior(other) if other != id) {
                return Err("departure side refers to a different boundary");
            }
        }
        if departure.is_some_and(|given| given != region) {
            return Err("departure side contradicts launch position");
        }
        Ok(region)
    }

    /// Side for the next segment after an accepted surface event. incoming is the
    /// propagation direction and hit.normal is the outward geometric normal.
    pub fn region_after_event(
        &self,
        current: MediumRegion,
        hit: &Hit,
        incoming: Vector3<f64>,
        event: impl Into<ScatterType>,
    ) -> Result<MediumRegion, &'static str> {
        self.active_medium(current)?;
        let object = self
            .objects
            .get(hit.object_id)
            .ok_or("invalid hit object")?;
        if object.medium_interface.is_none() {
            return Ok(current);
        }
        let facing = incoming.dot(&hit.normal);
        if !facing.is_finite() || facing == 0.0 {
            return Err("undefined dielectric incidence side");
        }
        let inside = MediumRegion::Interior(hit.object_id);
        let (incident, transmitted) = if facing < 0.0 {
            (MediumRegion::Exterior, inside)
        } else {
            (inside, MediumRegion::Exterior)
        };
        if incident != current {
            return Err("incident region contradicts the geometric normal");
        }
        match event.into() {
            ScatterType::SpecularReflection => Ok(current), // includes TIR
            ScatterType::SpecularTransmission => Ok(transmitted),
            _ => Err("expected a dielectric interface event"),
        }
    }
}
