use super::super::{offset_ray_origin, path::MAX_DELTA_EVENTS};
use super::{SpectralPathState, lighting};
use crate::{
    geometry::ray::Ray,
    material::unit_vector,
    optics::{emission::rgb_emission, iormode::IorMode},
    scene::Scene,
};
use rand::Rng;
use std::error::Error;

/// 相同的第三次连续连接后 RR 规则；只补偿之后的吞吐量，不缩放已累计的辐亮度。
pub(super) fn russian_roulette(
    state: &mut SpectralPathState,
    rng: &mut dyn rand::RngCore,
) -> Result<bool, Box<dyn Error>> {
    state.validate()?;
    if state.non_delta_bounces < 3 || state.non_delta_bounces >= state.max_non_delta_bounces {
        return Ok(true);
    }
    if state.throughput == 0.0 {
        return Ok(false);
    }
    let p = state.throughput.clamp(0.05, 0.95);
    if rng.random::<f64>() >= p {
        return Ok(false);
    }
    state.throughput /= p;
    if !state.throughput.is_finite() {
        return Err("光谱 RR 补偿溢出".into());
    }
    Ok(true)
}

/// 全路径固定 λ；仅传输标量 Lλ 和 throughput，CIE 和 1/pλ 留在相机像素积分阶段。
/// 不查询已经对波长积分的 XYZ 焦散贴图，避免把 XYZ 误当作单波长数据。
/// prefix_rng 仅用于首次连续顶点前的 Delta；environment_rng 仅用于首次环境 NEE。
/// 后续 BSDF、Delta、环境与 RR 使用 continuation_rng，深度变化不消耗上述前缀流。
pub fn trace_spectral_camera_path(
    scene: &Scene,
    state: &mut SpectralPathState,
    ior_mode: IorMode,
    prefix_rng: &mut dyn rand::RngCore,
    environment_rng: &mut dyn rand::RngCore,
    continuation_rng: &mut dyn rand::RngCore,
) -> Result<(), Box<dyn Error>> {
    state.validate()?;
    crate::config::sceneconfig::validate_diffuse_bounces(scene.max_diffuse_bounces)?;
    if scene.environment_samples == 0 {
        return Err("环境样本数必须为正".into());
    }
    loop {
        state.validate()?;
        if state.throughput == 0.0 {
            break;
        }
        let Some(hit) = scene.intersect(&state.ray) else {
            let emitted = if !state.has_non_delta_vertex {
                Some(
                    scene
                        .background_radiance(state.ray.direction)
                        .ok_or("光谱背景无效")?,
                )
            } else if state.last_scatter.is_some_and(|e| e.is_delta()) {
                scene
                    .environment
                    .as_ref()
                    .map(|env| env.radiance(state.ray.direction).ok_or("光谱环境逃逸无效"))
                    .transpose()?
            } else {
                None
            };
            if let Some(rgb) = emitted {
                state.radiance += state.throughput
                    * rgb_emission(rgb, state.wavelength_nm).ok_or("发光光谱重建失败或溢出")?;
            }
            break;
        };
        let material = hit
            .material_id
            .and_then(|id| scene.material(id))
            .ok_or("光谱路径命中未绑定材质的物体")?;
        if !material.supports_spectral() {
            return Err("材质不支持单波长光谱采样（GGX RGB eta/k 不可直接用于光谱模式）".into());
        }
        let normal = unit_vector(hit.normal).ok_or("光谱路径法线无效")?;
        let wo = -state.ray.direction;
        if material.has_non_delta() {
            if normal.dot(&wo) <= 0.0 && !material.has_delta() {
                break;
            }
            let scale = scene
                .ground_reflectance_scale(&hit)
                .ok_or("光谱棋盘格坐标无效")?;
            let direct = lighting::direct_sun(scene, &hit, material, wo, state.wavelength_nm)?;
            let environment = lighting::environment_lighting(
                scene,
                &hit,
                material,
                wo,
                state.wavelength_nm,
                ior_mode,
                if state.has_non_delta_vertex || state.non_delta_bounces > 0 {
                    &mut *continuation_rng
                } else {
                    &mut *environment_rng
                },
            )?;
            state.radiance += state.throughput * (direct + environment) * scale;
        }
        state.has_non_delta_vertex |= material.has_non_delta();
        let continuous_budget = state.non_delta_bounces < state.max_non_delta_bounces;
        let delta_budget = state.delta_events() < MAX_DELTA_EVENTS;
        if (!continuous_budget || !material.has_non_delta())
            && (!delta_budget || !material.has_delta())
        {
            break;
        }
        if material.has_non_delta()
            && continuous_budget
            && !russian_roulette(state, continuation_rng)?
        {
            break;
        }
        let sampling_rng: &mut dyn rand::RngCore =
            if state.has_non_delta_vertex || state.non_delta_bounces > 0 {
                &mut *continuation_rng
            } else {
                &mut *prefix_rng
            };
        let Some(sample) =
            material.sample_spectral(wo, normal, state.wavelength_nm, ior_mode, sampling_rng)
        else {
            break;
        };
        if sample.event.is_delta() != sample.pdf.is_none()
            || sample.pdf.is_some_and(|p| !p.is_finite() || p <= 0.0)
            || !sample.weight.is_finite()
            || sample.weight < 0.0
            || sample.event.is_delta() && !material.has_delta()
            || !sample.event.is_delta() && !material.has_non_delta()
        {
            return Err("光谱 BSDF 采样记录无效".into());
        }
        if sample.event.is_delta() && !delta_budget
            || !sample.event.is_delta() && !continuous_budget
        {
            break;
        }
        let wi = unit_vector(sample.wi).ok_or("光谱 BSDF 采样方向无效")?;
        if !sample.event.is_delta() && (normal.dot(&wi) <= 0.0 || normal.dot(&wo) <= 0.0) {
            break;
        }
        let scale = scene
            .ground_reflectance_scale(&hit)
            .ok_or("光谱棋盘格坐标无效")?;
        if !state.scatter(sample.event, sample.weight * scale)? {
            break;
        }
        let side = if normal.dot(&wi) >= 0.0 { 1.0 } else { -1.0 };
        state.ray =
            Ray::new(offset_ray_origin(hit.coords, normal, side), wi).ok_or("光谱续追射线无效")?;
    }
    state.validate()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Vector3;

    struct Fixed {
        value: u64,
        draws: usize,
    }
    impl rand::RngCore for Fixed {
        fn next_u64(&mut self) -> u64 {
            self.draws += 1;
            self.value
        }
        fn next_u32(&mut self) -> u32 {
            self.next_u64() as u32
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for byte in dest {
                *byte = self.next_u64() as u8;
            }
        }
    }

    #[test]
    fn roulette_compensates_only_future_scalar_throughput_and_skips_terminal_vertices() {
        let ray = Ray::new(Vector3::zeros(), Vector3::z()).unwrap();
        for (throughput, p) in [(0.01, 0.05), (0.2, 0.2), (5.0, 0.95)] {
            let mut state = SpectralPathState::new(ray, 550.0, 4).unwrap();
            state.path_depth = 3;
            state.non_delta_bounces = 3;
            state.has_non_delta_vertex = true;
            state.throughput = throughput;
            state.radiance = 0.123;
            let mut rng = Fixed { value: 0, draws: 0 };
            assert!(russian_roulette(&mut state, &mut rng).unwrap());
            assert!((state.throughput - throughput / p).abs() < 1e-13);
            assert_eq!(state.radiance, 0.123);
            assert_eq!(rng.draws, 1);
            state.max_non_delta_bounces = 3;
            assert!(russian_roulette(&mut state, &mut rng).unwrap());
            assert_eq!(rng.draws, 1);
        }
        let mut state = SpectralPathState::new(ray, 550.0, 4).unwrap();
        state.path_depth = 3;
        state.non_delta_bounces = 3;
        state.throughput = 0.3;
        assert!(
            !russian_roulette(
                &mut state,
                &mut Fixed {
                    value: u64::MAX,
                    draws: 0
                }
            )
            .unwrap()
        );
        assert_eq!(state.throughput, 0.3);
    }
}
