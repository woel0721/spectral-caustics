//! JSON 配置的统一入口：序列化、文件读写和配置数值校验。
//! 这里只返回配置数据，不创建 Scene、Render 或任何 Hittable 实例。

pub mod cameraconfig;
pub mod rendersetting;
pub mod sceneconfig;

use self::{rendersetting::RenderSetting, sceneconfig::SceneConfig};
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, path::Path};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub render: RenderSetting,
    pub scene: SceneConfig,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        self.render.validate()?;
        self.scene.validate()?;
        Ok(())
    }

    pub fn from_json(json: &str) -> Result<Self, Box<dyn Error>> {
        let config: Self = serde_json::from_str(json)?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_json(&self) -> Result<String, Box<dyn Error>> {
        // 先校验，避免 NaN/无穷值被 JSON 序列化为 null，或写入不可使用的配置。
        self.validate()?;
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
        // 先完成校验和序列化，再写文件；配置非法时不覆盖已有文件。
        let json = self.to_json()?;
        fs::write(path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::sceneconfig::ObjectConfig, optics::iormode::IorMode};
    use serde_json::json;
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temporary_path() -> PathBuf {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "spectral-config-{}-{time}.json",
            std::process::id()
        ))
    }

    #[test]
    fn checked_in_json_matches_current_defaults_and_roundtrips() {
        let config = Config::from_json(include_str!("../../configs/default.json")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.scene.objects.len(), 2);
        assert!(matches!(
            config.scene.objects[0],
            ObjectConfig::Prism { .. }
        ));
        assert_eq!(
            config.scene.objects[1],
            ObjectConfig::Ground { height_m: 0.0 }
        );
        assert_eq!(config.scene.light.total_flux, 8.0);
        assert_eq!(
            Config::from_json(&config.to_json().unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn finite_geometry_coordinates_roundtrip_without_losing_f64_bits() {
        let mut config = Config::default();
        for height_m in [
            -0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            0.12345678901234566,
            1.0000000000000002,
            f64::MAX,
        ] {
            config.scene.objects[1] = ObjectConfig::Ground { height_m };
            let loaded = Config::from_json(&config.to_json().unwrap()).unwrap();
            let ObjectConfig::Ground { height_m: actual } = loaded.scene.objects[1] else {
                panic!("地面类型必须在 JSON 往返后保留");
            };
            assert_eq!(actual.to_bits(), height_m.to_bits());
        }
    }

    #[test]
    fn changed_settings_and_both_modes_survive_json_file_roundtrip() {
        let path = temporary_path();
        assert!(Config::load(&path).is_err());
        let mut config = Config::default();
        config.render.width = 96;
        config.render.height = 64;
        config.render.seed = 7;
        config.render.thread_count = Some(4);
        config.render.output_path = PathBuf::from("output/中文 焦散.png");
        config.scene.light.height_m = 4.0;
        config.scene.camera.position = [-2.0, 3.0, 4.0];
        config.scene.camera.target = [0.0, 0.0, 1.2];
        config.scene.camera.fov_y_degrees = 60.0;
        config.scene.objects[1] = ObjectConfig::Ground { height_m: -1.0 };
        for (mode, name) in [
            (IorMode::Constant, "constant"),
            (IorMode::Dispersion, "dispersion"),
        ] {
            config.render.mode = mode;
            config.save(&path).unwrap();
            let json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(json["render"]["mode"], name);
            assert_eq!(Config::load(&path).unwrap(), config);
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_render_parameters_are_rejected_before_use() {
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/default.json")).unwrap();
        for (field, value) in [
            ("width", json!(0)),
            ("height", json!(0)),
            ("samples_per_pixel_axis", json!(0)),
            ("width", json!(-1)),
            ("wavelength_min_nm", json!(0.0)),
            ("wavelength_max_nm", json!(380.0)),
            ("wavelength_step_nm", json!(0.0)),
            ("wavelength_step_nm", json!(1000.0)),
            ("exposure", json!(-1.0)),
            ("thread_count", json!(0)),
            ("max_spectral_blocks", json!(0)),
            ("output_path", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["render"][field] = value;
            assert!(Config::from_json(&invalid.to_string()).is_err(), "{field}");
        }
        let mut config = Config::default();
        config.render.width = usize::MAX;
        assert!(config.validate().is_err());
        config = Config::default();
        config.render.wavelength_step_nm = f64::from_bits(1);
        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_scene_numbers_and_regions_are_rejected() {
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/default.json")).unwrap();
        for direction in [json!([0.0, 0.0, 0.0]), json!([1e308, 0.0, 0.0])] {
            let mut invalid = original.clone();
            invalid["scene"]["light"]["direction"] = direction;
            assert!(Config::from_json(&invalid.to_string()).is_err());
        }
        for pointer in ["/scene/light/aperture_max_m", "/scene/receiver/max_m"] {
            let mut invalid = original.clone();
            *invalid.pointer_mut(pointer).unwrap() = json!([-2.0, 2.0]);
            assert!(Config::from_json(&invalid.to_string()).is_err());
        }
        let mut invalid = original.clone();
        invalid["scene"]["background_linear_rgb"] = json!([-0.1, 0.0, 0.0]);
        assert!(Config::from_json(&invalid.to_string()).is_err());
        invalid = original.clone();
        invalid["scene"]["light"]["total_flux"] = json!(-1.0);
        assert!(Config::from_json(&invalid.to_string()).is_err());
        invalid = original;
        invalid["scene"]["camera"]["fov_y_degrees"] = json!(180.0);
        assert!(Config::from_json(&invalid.to_string()).is_err());
    }

    #[test]
    fn unknown_fields_types_missing_fields_and_bad_json_are_reported() {
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/default.json")).unwrap();
        for pointer in [
            "",
            "/render",
            "/scene",
            "/scene/camera",
            "/scene/light",
            "/scene/receiver",
            "/scene/objects/0",
        ] {
            let mut invalid = original.clone();
            invalid.pointer_mut(pointer).unwrap()["typo"] = json!(1);
            assert!(
                Config::from_json(&invalid.to_string()).is_err(),
                "{pointer}"
            );
        }
        let mut invalid = original.clone();
        invalid["scene"]["objects"][0]["type"] = json!("sphere");
        assert!(Config::from_json(&invalid.to_string()).is_err());
        invalid = original;
        invalid["render"].as_object_mut().unwrap().remove("width");
        assert!(Config::from_json(&invalid.to_string()).is_err());
        for json in ["", "{", "null", "{}"] {
            assert!(Config::from_json(json).is_err());
        }
    }

    #[test]
    fn invalid_camera_values_and_missing_fields_are_rejected() {
        let original: serde_json::Value =
            serde_json::from_str(include_str!("../../configs/default.json")).unwrap();
        for (field, value) in [
            ("position", json!([0.0, 0.0, 1.3])), // position == target。
            ("position", json!([1e308, 1e308, 1e308])), // 模长溢出。
            ("target", json!([3.0, -5.0, 3.0])),
            ("up", json!([0.0, 0.0, 0.0])),
            ("up", json!([-3.0, 5.0, -1.7])), // up 平行于观察方向。
            ("position", json!([0.0, 0.0])),
            ("target", json!("origin")),
            ("fov_y_degrees", json!(0.0)),
            ("fov_y_degrees", json!(-1.0)),
            ("fov_y_degrees", json!(180.0)),
        ] {
            let mut invalid = original.clone();
            invalid["scene"]["camera"][field] = value;
            assert!(Config::from_json(&invalid.to_string()).is_err(), "{field}");
        }
        for field in ["position", "target", "up", "fov_y_degrees"] {
            let mut invalid = original.clone();
            invalid["scene"]["camera"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(Config::from_json(&invalid.to_string()).is_err(), "{field}");
        }
        let mut invalid = original;
        invalid["scene"].as_object_mut().unwrap().remove("camera");
        assert!(Config::from_json(&invalid.to_string()).is_err());
    }

    #[test]
    fn non_finite_values_are_not_serialized_or_written_over_existing_files() {
        let path = temporary_path();
        fs::write(&path, "keep existing config").unwrap();
        let mut config = Config::default();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            config.render.exposure = value;
            assert!(config.to_json().is_err());
            assert!(config.save(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), "keep existing config");
        }
        config = Config::default();
        config.scene.objects[1] = ObjectConfig::Ground { height_m: f64::NAN };
        assert!(config.to_json().is_err());
        config.scene.objects[0] = ObjectConfig::Prism {
            vertices_m: [[f64::INFINITY; 3]; 6],
        };
        assert!(config.to_json().is_err());
        for field in ["position", "target", "up", "fov_y_degrees"] {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                config = Config::default();
                match field {
                    "position" => config.scene.camera.position[0] = value,
                    "target" => config.scene.camera.target[0] = value,
                    "up" => config.scene.camera.up[0] = value,
                    _ => config.scene.camera.fov_y_degrees = value,
                }
                assert!(config.to_json().is_err());
                assert!(config.save(&path).is_err());
                assert_eq!(fs::read_to_string(&path).unwrap(), "keep existing config");
            }
        }
        fs::remove_file(path).unwrap();
    }
}
