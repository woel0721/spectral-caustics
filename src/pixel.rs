use nalgebra::Vector3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pixel {
    pub coordinate: Vector3<f64>,
    pub color: Vector3<f32>,
}

impl Pixel {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            coordinate: Vector3::new(x, y, z),
            color: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    pub fn is_finite(&self) -> bool {
        self.coordinate.iter().all(|v| v.is_finite())
    }

    pub fn set_color(&mut self, color: Vector3<f32>) {
        if !color.iter().all(|v| v.is_finite()) {
            panic!("颜色值必须是有限数");
        }
        self.color = color;
    }
}
