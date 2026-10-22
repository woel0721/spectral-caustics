//! 单条光线：空气 -> 玻璃 -> 空气 -> 地面。

use nalgebra::Vector3;
use spectral_caustics::{ground::Ground, object::Hittable, prism::Prism, ray::Ray, scene::refract};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let prism = Prism::new().ok_or("三棱镜构造失败")?;
    let ground = Ground::new(0.0).ok_or("地面构造失败")?;
    let air_ior = 1.0;
    let glass_ior = 1.52;
    // 比求交的 1e-6 米阈值稍大，避免下一条光线再次命中刚离开的表面。
    let epsilon = 1e-5;

    // 1. 从空气中向下发射，选 x = 0.25，避开顶棱。
    let incoming_ray = Ray::new(
        Vector3::new(0.25, 0.2, 3.0),
        Vector3::new(0.0, 0.0, -1.0),
        0.0,
    )
    .ok_or("入射光线无效")?;
    let entry = prism.intersect(&incoming_ray).ok_or("没有命中玻璃")?;

    // 2. 空气 -> 玻璃：朝外法线正好朝向入射介质（空气）。
    let inside_direction =
        refract(incoming_ray.direction, entry.normal, air_ior, glass_ior).ok_or("入射折射失败")?;
    let inside_ray = Ray::new(
        entry.coords - entry.normal * epsilon, // 沿法线反向，移到玻璃内侧。
        inside_direction,
        0.0,
    )
    .ok_or("玻璃内光线无效")?;

    // 3. 在同一个三棱镜内继续求交，找到最近的出射面。
    let exit = prism.intersect(&inside_ray).ok_or("没有找到出射面")?;

    // 4. 玻璃 -> 空气：入射介质在玻璃内部，因此传入朝内的 -exit.normal。
    let outside_direction = refract(inside_ray.direction, -exit.normal, glass_ior, air_ior)
        .ok_or("出射发生全反射，当前示例终止光线")?;
    let outside_ray = Ray::new(
        exit.coords + exit.normal * epsilon, // 沿朝外法线，移到玻璃外侧。
        outside_direction,
        0.0,
    )
    .ok_or("出射光线无效")?;

    // 5. 出射后与地面求交，查看最终落点。
    let ground_hit = ground
        .intersect(&outside_ray)
        .ok_or("出射光线没有落到地面")?;
    println!("入射点：{:?}", entry.coords);
    println!("玻璃内方向：{:?}", inside_ray.direction);
    println!("出射点：{:?}", exit.coords);
    println!("出射方向：{:?}", outside_ray.direction);
    println!("地面落点：{:?}", ground_hit.coords);
    Ok(())
}
