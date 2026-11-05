use super::scene::EnvironmentMisHeuristic;
use crate::optics::dispersion::IorMode;
use crate::{
    camera::Camera, caustics::GroundReceiver, material::checkerboard::Checkerboard, scene::Scene,
};
pub struct CameraOptions {
    pub environment_samples: usize,
    pub light_samples: usize,
    pub environment_mis_heuristic: EnvironmentMisHeuristic,
    pub max_diffuse_bounces: usize,
}
pub struct PhotonOptions {
    pub receiver: GroundReceiver,
    pub caustics_object_id: Option<usize>,
    pub ground_id: Option<usize>,
}
pub struct RenderJob {
    pub scene: Scene,
    pub camera: Camera,
    pub camera_options: CameraOptions,
    pub photon: PhotonOptions,
    pub ground_checkerboard: Option<Checkerboard>,
    #[cfg(feature = "diagnostics")]
    pub diagnostics: crate::diagnostics::Settings,
}

impl RenderJob {
    pub fn select_caustics_object(&mut self, object_id: usize) -> Result<(), String> {
        let material = self
            .scene
            .objects
            .get(object_id)
            .map(|object| object.material_id())
            .map(|id| self.scene.material(id))
            .ok_or("焦散物体必须绑定理想 Dielectric 材质")?;
        if material.ior().is_none() || self.photon.ground_id == Some(object_id) {
            return Err("焦散物体必须是接收地面以外的理想 Dielectric".into());
        }
        if self
            .photon
            .caustics_object_id
            .is_some_and(|id| id != object_id)
        {
            return Err(
                "当前只支持一个焦散物体，请仅为一个 Sphere/Prism/VerticalPrism 启用 caustics"
                    .into(),
            );
        }
        self.photon.caustics_object_id = Some(object_id);
        Ok(())
    }
    pub(crate) fn photon_ior(
        &self,
        object_id: usize,
        wavelength_nm: Option<f64>,
        mode: IorMode,
    ) -> Option<f64> {
        let object = self.scene.objects.get(object_id)?;
        self.scene
            .material(object.material_id())
            .photon_ior(wavelength_nm, mode)
    }
    pub(crate) fn supports_ideal_caustics(&self, caustics_object_id: usize) -> bool {
        self.scene
            .objects
            .get(caustics_object_id)
            .is_some_and(|o| self.scene.material(o.material_id()).ior().is_some())
    }
}
