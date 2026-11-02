use nalgebra::Vector3;
use rand::{SeedableRng, rngs::StdRng};
use spectral_caustics::{
    config::rendersetting::RenderSetting,
    config::{
        materialconfig::MaterialConfig,
        sceneconfig::{ObjectConfig, SceneConfig},
    },
    material::{self, ScatterType},
    render::caustics::render_spectral_lighting,
    scene::Scene,
};

#[test]
fn mirror_json_roundtrip_builds_trait_object_and_validates_parameters() {
    let config: MaterialConfig =
        serde_json::from_str(r#"{"type":"mirror","reflectance":[0.2,0.5,0.9]}"#).unwrap();
    config.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<MaterialConfig>(&serde_json::to_string(&config).unwrap()).unwrap(),
        config
    );
    let material = material::from_config(&config).unwrap();
    let s = material
        .sample(Vector3::z(), Vector3::z(), &mut StdRng::seed_from_u64(42))
        .unwrap();
    assert_eq!(s.weight, Vector3::new(0.2, 0.5, 0.9));
    assert_eq!(s.event, ScatterType::SpecularReflection);
    for value in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
        let bad = MaterialConfig::Mirror {
            reflectance: [value, 0.5, 0.5],
        };
        assert!(bad.validate().is_err());
        assert!(material::from_config(&bad).is_err());
    }
    for json in [
        r#"{"type":"mirror"}"#,
        r#"{"type":"mirror","reflectance":[1,1,1],"roughness":0.3}"#,
    ] {
        assert!(serde_json::from_str::<MaterialConfig>(json).is_err());
    }
}

#[test]
fn mirror_prism_does_not_generate_the_ideal_glass_spectral_caustic() {
    let mut config = SceneConfig {
        environment: None,
        ..Default::default()
    };
    for object in &mut config.objects {
        if let ObjectConfig::Prism { material, .. } = object {
            *material = MaterialConfig::Mirror {
                reflectance: [0.9; 3],
            };
        }
    }
    let scene = Scene::from_config(&config).unwrap();
    let settings = RenderSetting {
        width: 32,
        height: 32,
        samples_per_pixel_axis: 2,
        ..Default::default()
    };
    let lighting =
        render_spectral_lighting(&scene, scene.prism_id, scene.ground_id.unwrap(), &settings)
            .unwrap();
    assert!(
        lighting
            .caustics
            .pixels
            .iter()
            .all(|p| *p == Vector3::zeros())
    );
    assert!(lighting.direct.pixels.iter().any(|p| p.norm() > 0.0));
}
