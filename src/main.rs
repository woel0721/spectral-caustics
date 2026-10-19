//! Day 2 光线示例：方向归一化，以及与地面、单个三角形分别求交。

mod framebuffer;
mod hit;
mod object;
mod ray;
mod render;
mod triangle;

use nalgebra::Vector3;
use object::Hittable;

fn main() {
    let ray = ray::Ray::new(
        Vector3::new(0.0, 0.0, 2.0),
        Vector3::new(0.0, 0.0, -5.0),
        0.0,
    )
    .expect("光线参数非法");

    println!("归一化后的方向：{:?}", ray.direction);
    println!("方向分量的倒数：{:?}", ray.direction_inv);
    println!("光线携带的时间：{}", ray.t);

    // map 将 Some(distance) 转为 Some(hit)，未命中的 None 保持不变。
    let hit = ray.intersect_ground().map(|distance| hit::Hit {
        coords: ray.at(distance),
        normal: Vector3::new(0.0, 0.0, 1.0),
        distance,
        ..hit::Hit::default()
    });

    println!("是否命中：{}", hit.is_some());

    match hit {
        Some(hit) => {
            println!("命中地面的距离：{} 米", hit.distance);
            println!("地面交点：{:?}", hit.coords);
            println!("地面朝外法线：{:?}", hit.normal);
            println!("是否正面命中：{}", ray.direction.dot(&hit.normal) < 0.0);
        }
        None => println!("光线没有有效的地面交点"),
    }

    // 单独测试 z = 1 的三角形；当前尚未将多个物体组合为场景。
    let triangle = triangle::Triangle::new(
        Vector3::new(-1.0, -1.0, 1.0),
        Vector3::new(1.0, -1.0, 1.0),
        Vector3::new(0.0, 1.0, 1.0),
    )
    .expect("三角形顶点非法");

    match triangle.intersect(&ray) {
        Some(hit) => {
            println!("三角形交点距离：{} 米", hit.distance);
            println!("三角形交点：{:?}", hit.coords);
            println!("三角形法线：{:?}", hit.normal);
        }
        None => println!("光线没有有效的三角形交点"),
    }
}
